# Phase 4.5 — React Terminal Pane Subscribes to Frame Stream

**Branch:** `feat/session-stream` (continues from Phase 4 H7.3)
**Status:** PLANNED — starting 2026-04-21
**Strategic goal:** the Tauri app's React terminal pane renders from
the daemon's Session Stream WebSocket as a **drop-in replacement**
for the existing alacritty-grid pipeline. Feature-flag reversible
per project via `use_session_stream`. Once stable for one release,
Phase 5 deletes the alacritty path.
**Engineering stance:** complete, not quick. Option B scope: full
drop-in, including cursor semantics + ANSI styling + input path +
resize + copy/paste. No read-only demo shortcut.

---

## Why this matters (the payoffs the Phase 4.5 demo unlocks)

- **First user-visible proof** of the Session Stream pipeline. Every
  keystroke a user types, every byte a model emits, runs through the
  pipeline we shipped in Phases 1–4.
- **De-risks Phase 5.** Phase 5's entire scope is "delete alacritty."
  Doing so safely requires a Frame-driven pane that handles every
  real-world TUI at parity. Phase 4.5 IS the validation gate for
  Phase 5.
- **Thin-client symmetry.** Whatever rendering logic the desktop
  Tauri pane uses post-4.5 can be reused by a future iOS companion,
  web viewer, or CLI-attach tool — they all subscribe to the same
  `/cli/sessions/subscribe` WebSocket.

---

## What already exists (from Phases 1–4)

| Primitive | Module | Shape |
|---|---|---|
| `SessionEntry` + broadcast + replay ring | `k2so_core::session::entry` | Daemon-owned session state |
| `Frame` enum | `k2so_core::session::frame` | 5 variants: Text, CursorOp, SemanticEvent, AgentSignal, RawPtyFrame |
| `/cli/sessions/subscribe` WS | `k2so_daemon::sessions_ws` | Already proven with `sessions_ws_integration.rs` — 6 tests green |
| `/cli/sessions/spawn` POST | `k2so_daemon::awareness_ws` | F2; spawn a Session Stream session |
| `/cli/terminal/write` | `k2so_daemon::terminal_routes` | H1; write bytes to a session by id |
| `/cli/terminal/read` | `k2so_daemon::terminal_routes` | H1; read Frame::Text lines (useful for debugging) |
| `use_session_stream` project setting | `k2so_core::agents::settings` | G6 |
| `heartbeat.{port,token}` | `k2so_daemon/src/main.rs` | H7 — daemon owns eagerly |

**Nothing on the React side exists yet.** `AlacrittyTerminalView.tsx`
(922 lines) is the current terminal component and listens to Tauri
events `terminal:grid:<terminalId>` bearing `GridUpdate` (alacritty's
view-model). Phase 4.5 introduces a parallel component,
`<SessionStreamView />`, that subscribes to the daemon's WebSocket
and handles every rendering concern alacritty does today.

---

## The 11 commits (I1–I11)

Each commit leaves the tree green and independently revertable.

### I1 — Tauri command `daemon_ws_url()`

**Scope:** new `#[tauri::command]` fn in `src-tauri/` that reads
`~/.k2so/heartbeat.port` + `~/.k2so/heartbeat.token` and returns
`{port: number, token: string}`. The React side composes the full
`ws://127.0.0.1:<port>/cli/sessions/subscribe?session=<uuid>&token=<token>`
URL; keeping composition on the JS side keeps the command narrow.

**Tests:** unit test for the Rust command's file-read path.

**Commit:** `I1 (phase 4.5): daemon_ws_url command exposes port + token`

---

### I2 — TypeScript Frame types + SessionStreamClient

**Scope:** two new modules:
- `src/renderer/types/frame.ts` — TS mirror of Rust's `Frame` enum.
  Matches the serde-tagged JSON shape byte-for-byte. Same for
  `Style`, `CursorOp`, `SemanticEvent`, `AgentSignal`.
- `src/renderer/lib/sessionStream.ts` — `SessionStreamClient` class.
  Opens a WS, parses `session:ack` / `session:frame` /
  `session:error` envelopes, emits typed callbacks. Reconnect on
  drop with exponential backoff. Cleanup on dispose.

**Tests:** vitest (if present) or a manual smoke harness; mock the
WS with a local test server.

**Commit:** `I2 (phase 4.5): Frame types + SessionStreamClient TS primitive`

---

### I3 — `TerminalGrid` state machine

**Scope:** `src/renderer/lib/terminalGrid.ts` — pure state machine
that accepts `Frame` events and maintains:
- 2D cell buffer (rows × cols, each cell = char + style)
- Scrollback (circular, bounded ~10k lines)
- Cursor position + visibility
- Scroll region (top, bottom)
- Alternate screen buffer (for TUIs like `vim`, `less`)

Handles every `CursorOp` variant: CUP (position), EL/ED (erase line /
display), SU/SD (scroll up/down), DECSTBM (set scroll region), and
mode switches (alt screen, APP_CURSOR, etc.).

**Tests:** unit tests covering each CursorOp → grid transition.

**Commit:** `I3 (phase 4.5): TerminalGrid — state machine for Frame events`

---

### I4 — Style / SGR model

**Scope:** `src/renderer/lib/terminalStyle.ts` — parses Rust's
`Style` struct into fg/bg color (number or palette index) + attribute
bitmask. Compatible with the existing `ATTR_*` constants in
`AlacrittyTerminalView.tsx` so the renderer can be shared when
possible.

**Tests:** unit tests for each SGR branch (basic 16, 256, truecolor,
reset, bold/italic/underline).

**Commit:** `I4 (phase 4.5): Style model — SGR decoding to fg/bg/attrs`

---

### I5 — `<SessionStreamView />` React component

**Scope:** `src/renderer/components/Terminal/SessionStreamView.tsx` —
takes `{sessionId, port, token, cols, rows}` props. Internally:
instantiates `SessionStreamClient` + `TerminalGrid`. Renders grid +
cursor via DOM spans, mirroring `renderLineSpans()` from
`AlacrittyTerminalView`. Subscribes to grid updates, batches via
`requestAnimationFrame`, unsubscribes on unmount.

No keyboard/mouse wiring yet — pure display.

**Tests:** component render test (render a grid snapshot, assert
DOM shape).

**Commit:** `I5 (phase 4.5): SessionStreamView — DOM render of Frame-driven grid`

---

### I6 — Keyboard input → `/cli/terminal/write`

**Scope:** wire the existing `src/renderer/lib/key-mapping.ts`
(212 lines, already parity with alacritty) to the daemon. Keydown
event → encode bytes → POST to `/cli/terminal/write?id=<uuid>&text=<urlencoded>`.
Mouse paste → same write endpoint (bracketed paste if the mode
flag is set).

Focus management: only active pane captures keys.

**Tests:** fake WS + mock daemon; assert write calls for a range of
key events.

**Commit:** `I6 (phase 4.5): keyboard input path — keydown → daemon write`

---

### I7 — Resize path

**Scope:** new daemon endpoint `/cli/sessions/resize?session=<id>&cols=N&rows=N`
in `k2so_daemon::sessions_ws` (or `terminal_routes`). Calls
`SessionStreamSession::resize()` which delegates to portable-pty.
React side uses ResizeObserver on the pane container; debounced
~100ms.

**Tests:** integration test: spawn → resize → read back `tput cols`
/ `tput lines` from the session.

**Commit:** `I7 (phase 4.5): /cli/sessions/resize + ResizeObserver wiring`

---

### I8 — Selection + copy/paste

**Scope:** mouse selection → DOM range → serialize to plain text
(strip ANSI) → navigator.clipboard.writeText. Paste → read clipboard
→ wrap in bracketed-paste escapes (if mode) → send via write path.

**Tests:** manual smoke (clipboard API is hard to unit-test).

**Commit:** `I8 (phase 4.5): mouse selection + copy/paste`

---

### I9 — Spawn flow for Session Stream sessions

**Scope:** when the user creates a new terminal tab AND the project
has `useSessionStream='on'`, spawn via daemon's `/cli/sessions/spawn`
(F2, already daemon-native) instead of Tauri's `terminal_create`
command. Response carries a `SessionId` UUID; plumb it into the
terminal tab metadata (Zustand store).

**Tests:** Tauri command + daemon round-trip.

**Commit:** `I9 (phase 4.5): session-stream spawn path for new terminals`

---

### I10 — AlacrittyTerminalView feature-flag integration

**Scope:** in `AlacrittyTerminalView.tsx` (or a new
`<TerminalView />` wrapper), branch on the project's
`useSessionStream` flag. When on, render `<SessionStreamView />`
with the session's UUID; when off, keep the existing alacritty path
unchanged.

Toggle via existing Zustand `useSettingsStore`. Live-switchable
without reload — switching sources terminates the current view and
creates the new one.

**Tests:** render snapshot with flag on/off, assert correct
component mounts.

**Commit:** `I10 (phase 4.5): feature-flag branch — SessionStream vs alacritty view`

---

### I11 — Phase 4.5 completion note + validation sweep

**Scope:** mirror phase-4 completion doc. Document: what works,
what's still alacritty-only (should be nothing real-world-visible),
how to toggle per project, how to roll back. Run the full test
battery one more time.

**Tests:** the battery — tier1/2/3 + cli-integration + daemon +
core + a new `crates/k2so-daemon/tests/sessions_ws_e2e.rs` that
spawns a real bash session, writes `echo hello`, reads back via
WS, asserts Frame::Text with "hello" appears.

**Commit:** `Phase 4.5 complete — React pane drives from Session Stream`

---

## Invariants preserved across Phase 4.5

1. **Subscribers never import alacritty types.** React side handles
   Frame events exclusively; no alacritty-originated types leak into
   `src/renderer/lib/terminalGrid.ts` or below.
2. **Feature flag gates consumer side only.** `useSessionStream='off'`
   keeps every byte flowing through the legacy alacritty path. Flag
   is per-project.
3. **Daemon is the sole HTTP server.** Tauri doesn't spin up any new
   listener — all communication is React → Tauri command →
   `DaemonClient` (HTTP) OR React → daemon direct (WebSocket).
4. **Heartbeat.{port,token} auth discipline.** Every WS URL carries
   the token in a query param; daemon rejects unauthenticated
   upgrades with 403.
5. **Every spawn still writes activity_feed.** Unchanged.

---

## Gotchas to watch for

- **Alt screen buffer.** `vim`, `less`, `htop` all swap to the
  alternate screen via CSI ?1049h/l. Must be modeled in the grid
  state or full-screen TUIs render garbage on the scrollback.
- **Cursor visibility.** CSI ?25h/l — show/hide cursor. Our grid
  needs this because cursor rendering is a prominent visual element.
- **Bracketed paste mode.** CSI ?2004h/l. Paste path must honor
  this or shells like zsh / fish reject long pastes.
- **UTF-8 multi-byte handling.** Frame::Text carries Vec<u8>. The
  TS side must decode with TextDecoder + buffer partial sequences
  across frames.
- **Replay burst at subscribe time.** Daemon's WS sends a
  `session:ack { replayCount }` then a rapid stream of the replay
  ring. We need a render path that can absorb thousands of frames
  without jank — batch via rAF, drop intermediate renders.
- **Focus stealing.** When a tab's pane is the React component, the
  keyboard handler must only fire when the pane is focused — the
  existing alacritty pane has this logic; port carefully.
- **Mouse mode (CSI ?1000h).** Shells like `tmux` enable mouse
  reporting. Option B requires forwarding mouse coords as
  escape sequences. **Scope call: defer to Phase 5 if it bloats 4.5
  past 11 commits.** Non-mouse TUIs work without it.

---

## Rollback

All new code lives in new files or behind the `useSessionStream`
feature flag. Rollback paths:

- **Per-project:** user sets `useSessionStream=off` → pane restores
  alacritty rendering on next tab open.
- **Emergency:** revert individual I-commits; each is self-contained.
- **Nuclear:** `git reset --hard v0.33.0` + rebuild. Flag-off build
  is still bit-for-bit v0.33.0.

---

## Ordering and parallelism

I1 must land first (URL discovery is a hard dep for every commit
after it). I2 → I3 → I4 are independent but benefit from landing
in order (I2's types feed I3's state machine, I3's state feeds I5's
render, I4's styles feed I5's render). I5 → I6 → I7 → I8 are all
pane-level concerns that can ship in any order once I5 is in. I9
depends on I5 being usable. I10 depends on everything else. I11
is the capstone.

Recommended order: linear I1 → I11. Each commit fully tested against
the existing battery before the next starts.

---

## Before starting

I1 starts now. Scope-check every commit against this plan; if any
grows past "cleanly bounded change," write down the deviation here.

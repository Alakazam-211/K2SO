# Canvas Plan — Session as Byte Stream, Client-Side vte

**Status:** addendum to `session-stream-and-awareness-bus.md`
**Date:** 2026-04-22
**Author:** Rosson (design), Coordinator (draft)

## 1. What this addendum is

A follow-on architecture document. It does **not** supersede
`session-stream-and-awareness-bus.md` — the original Session Stream
design (Frame stream, multi-device thin clients, Awareness Bus)
stays exactly as specified. This addendum describes a **second
subscription tier** that runs alongside the Frame stream and
provides pixel-perfect reflow, selection, and scrollback fidelity
for clients capable of hosting a local terminal emulator.

The two tiers coexist and serve different consumers:

| Tier | Subscribers | Best for |
|---|---|---|
| **Byte stream** (new) | Tauri-based clients (K2SO desktop today; Tauri-on-mobile later) | Pixel-perfect reflow, native selection, find-in-scrollback, same-width-or-wider viewing |
| **Frame stream** (existing) | Mobile companion, web attach, CLI viewers, T0.5 recognizers, T1 stream-json | Multi-device reflow at *native* client width, semantic rendering, no-vte environments |

Clients choose based on capability, not mandate.

## 2. Why we need it

The original PRD explicitly rejected a unified daemon-side grid to
keep multi-device reflow clean (a shared grid bakes width into
every subscriber's view). That call was correct for the daemon
layer. But it left **each client** responsible for maintaining
its own grid/scrollback structure and all the terminal invariants
that come with it — and Kessel's homegrown TypeScript
`TerminalGrid` has repeatedly shown us where that leaks:

- Scrollback doesn't reflow on resize.
- Selection is anchored to DOM nodes, not content.
- The grow-then-shrink seam has to be hand-computed and it's easy
  to get wrong (we did).
- Every new terminal feature (find, Cmd+F, bracketed search,
  wrap-at-width reflow, alt-screen toggle animation) is a
  re-implementation of work Alacritty solved years ago.

The Canvas Plan moves each client's grid into an actual terminal
emulator (`alacritty_terminal::Term`) and treats the daemon purely
as a byte broadcaster. We stop writing terminal features in
TypeScript and start consuming them from a proven crate.

## 3. The model

### 3.1 A Session is a byte stream

**Definition:** a Session is the ordered, append-only byte stream
from a single PTY lifetime, plus a small set of in-band semantic
markers the daemon injects. Every derived state (grid, scrollback,
Frame events, archive NDJSON) is a **projection** of the Session.

- The daemon **owns** the Session bytes.
- The daemon **writes** to it (PTY reader + APC markers).
- Subscribers **replay** from it (byte offset 0 → now → live).
- Reading does not consume; the ring is shared and persistent.
- Long-lived sessions overflow the in-memory ring (`REPLAY_CAP`) —
  the on-disk byte archive is the authoritative record for late
  attachers.

### 3.2 In-band APC markers

The daemon inserts semantic markers directly into the byte stream
using the reserved `k2so:` APC namespace (already defined in the
original PRD §"Reserved APC escape"). Format:

```
\x1b_k2so:<kind>:<json-payload>\x07
```

Defined markers for the Canvas Plan:

| Kind | Payload | Purpose |
|---|---|---|
| `grow_boundary` | `{target_cols, target_rows, grow_rows, reason}` | Fires at grow-settle. Tells the client to seal the grow-phase content into scrollback and resize the live Term to the target. See §4. |

Future markers land here as we need them. Format is extensible by
convention: new `kind`s ignored by old clients (pre-vte filter
strips them harmlessly).

### 3.3 The client pipeline

```
Session bytes → k2so-APC filter → alacritty vte::Processor → alacritty Term
                        ↓
                handle grow_boundary:
                  seal: push grid[0..cursor] into Term scrollback
                  reset: clear live grid at target size
                  resize: term.resize(target_cols, target_rows)
```

The APC filter runs **before** vte, extracts `k2so:` escapes,
performs their side effects on the Term, and passes the rest of
the bytes through unchanged. vte never sees `k2so:` escapes — they
are for the filter, not the emulator.

Everything downstream is Alacritty's job:
- Text, cursor movement, SGR, mode changes, alt-screen,
  scroll-region, wrap, bracketed paste, Unicode width — handled.
- Grid mutation, scrollback push on line-feed overflow, cursor
  state, selection tracking — handled.
- Reflow on `Term::resize` — **handled**. This is the key
  feature we've been missing.

## 4. Seam semantics

**Problem:** when the daemon SIGWINCHes the PTY from GROW_ROWS
down to the user's real rows, Claude's post-SIGWINCH repaint
begins with a `ClearScreen` that wipes whatever was in the visible
grid. If we simply pushed the top `(GROW_ROWS - target_rows)` rows
to scrollback and left the bottom `target_rows` as the live grid,
those bottom rows — which at grow-phase capture time contained
conversation content — get wiped by the repaint. Result: only
the topmost part of the conversation survives in scrollback.

**Ideal seam:** at the cursor's resting row when grow-settle fires.
Everything above the cursor is conversation content that Claude
painted during grow and will NOT repaint post-SIGWINCH — push it to
scrollback. Everything at and below the cursor is either blank or
about to be overwritten — discard it before Claude's ClearScreen
does.

**Implementation:** the daemon emits `grow_boundary` with the
cursor row and target dims at the moment of settle. Each client's
APC filter seals the grow phase:

```
contentRowCount = cursor.row + 1
for r in 0..contentRowCount:
    scrollback.push(grid[r])   // into Term's scrollback
grid = blank(target_cols, target_rows)
cursor = (0, 0)
term.resize(target_cols, target_rows)
```

After this, vte continues processing bytes into a clean live grid.
Claude's ClearScreen wipes nothing useful (the grid is already
blank). Claude's repaint lands in a fresh canvas. The grow-phase
conversation survives intact in scrollback.

**Determinism:** the seam is a pure function of the cursor at
grow_boundary time. Every subscriber, live or late, processes the
same byte stream with the same marker at the same offset and ends
up with the same final Term state.

## 5. Byte archive

The daemon persists the full byte stream to disk alongside the
existing Frame NDJSON archive. Format:

- Path: `<project>/.k2so/sessions/<session_id>.bytes`
- Format: raw bytes as written by the PTY reader, with APC
  markers inline. No framing, no length prefixes — it IS the
  Session.
- Rotation: never (a Session is a single PTY lifetime). When the
  session ends, the file is finalized.
- Retention: same policy as Frame NDJSON (project-scoped; evicted
  by existing archive cleanup when present).

**Replay endpoint:** `GET /cli/sessions/bytes?session=<uuid>&from=<offset>`
streams the byte archive from `offset` forward and upgrades to the
live broadcast tail. Equivalent to `/cli/sessions/subscribe` but
for bytes instead of Frames.

The broadcast channel for live bytes mirrors the existing
`SessionEntry::tx` pattern — a `tokio::sync::broadcast::Sender<Vec<u8>>`
sibling to the Frame sender.

## 6. Coexistence with the Frame stream

The Frame stream stays **fully alive** and unchanged. LineMux
continues to produce Frame events from the same PTY bytes; the
Frame broadcast channel continues to serve thin clients. The only
daemon-side change is additive: one more writer tap on the reader
thread that appends bytes to the byte ring + archive, plus the
APC marker injection points.

Consumer matrix after the Canvas Plan lands:

| Consumer | Subscribes to | Why |
|---|---|---|
| Kessel (K2SO desktop) | bytes | Pixel-perfect reflow at user's window width |
| Mobile companion | Frames (T0.5/T1) | Native-width reflow; no local vte |
| Web attach (future) | Frames | Same |
| Archive writer | Frames + bytes | Both projections persisted |
| Awareness Bus ingress | Frames (AgentSignal variant) | Semantic routing unchanged |

## 7. Honest limitations

### 7.1 Grow-phase is a moment capture for CUP-based TUIs

Claude (and other CUP-based TUIs like htop, less) don't emit line
feeds that push content off the bottom of the grid during normal
operation — they repaint in place via absolute cursor positioning.
This means **our scrollback only grows during grow-phase capture
or during line-feed-style output**. For Claude:

- Grow-phase on resume: captures the conversation Claude chose to
  render at GROW_ROWS (Claude caps its own render at ~60 rows).
- Post-boundary: Claude's UI updates don't add to scrollback.
  Conversation history beyond Claude's own rendering window is
  in Claude's SQLite, not in our byte stream.

**The PRD's answer to "unlimited semantic scrollback" is T1
stream-json**, not grid tricks. The Canvas Plan does not solve
this — it solves pixel-perfect fidelity for what we DO capture.
T1 migration is a separate, complementary track.

### 7.2 Byte stream doesn't fit narrower clients

A client at 40 cols viewing a session where the daemon painted at
120 cols can't reflow the byte stream — CUP sequences past col 40
get clamped, content is lost. Narrower-than-capture clients must
subscribe to the Frame stream (T0.5/T1 semantic events) or accept
clipped rendering. This matches the PRD's original design
rationale.

### 7.3 Per-pane Term cost

Each Kessel pane allocates a Term with its configured scrollback
(~5000 lines × cols cells). Rough order of magnitude: 1-2 MB per
pane. Acceptable for desktop; would be prohibitive on a phone
(another reason mobile stays on the Frame stream).

## 8. Phase plan

Six phases, ordered so each one is shippable and testable on its
own. Commit at every phase boundary as a save-game.

### Phase 1 — Seam fix in current TerminalGrid (~half day)

Before touching architecture, fix the seam in the existing
TypeScript TerminalGrid so today's implementation preserves the
full conversation to scrollback. Establishes a known-good baseline
where the ring → scrollback flow visibly works end-to-end before
we start moving it around.

- Add `TerminalGrid.sealGrowPhase(contentRows, cols, rows)` that
  pushes top-N rows to scrollback and resets live grid to blank
  at target size.
- SessionStreamView's `grow_boundary` handler calls
  `sealGrowPhase(cursor.row + 1, target_cols, target_rows)`
  instead of the current `trimRows + resize` pattern.
- Same update for the 3-second fallback handler.
- Grid test covering the seal behavior.

**Success criteria:** `claude --resume 0eacbe50-d6a1-40f2-9070-16aca988b7db`
in Cortana shows the full Kestrel Point story in scrollback, not
just the first few rows.

### Phase 2 — Byte log + broadcast (~2 days)

Daemon gets a byte-stream tap. Frame stream is unchanged.

- `SessionEntry` gains a `bytes_tx: broadcast::Sender<Vec<u8>>`
  sibling channel + `bytes_ring: Arc<Mutex<VecDeque<Vec<u8>>>>`.
- `reader_loop` writes each PTY chunk to both LineMux and the
  byte ring.
- `session::archive::spawn` adds a byte-archive writer task
  that flushes to `<project>/.k2so/sessions/<id>.bytes`.
- New HTTP/WS endpoint `/cli/sessions/bytes` streams from offset.
- Tests: replay from offset 0, replay from mid-offset, live tail.

**Success criteria:** `curl /cli/sessions/bytes?session=<uuid>&from=0`
outputs exactly the bytes Claude wrote, in order.

### Phase 3 — APC marker plumbing (~1 day)

- `grow_boundary` emission switches from `Frame::SemanticEvent`
  (which lives in the Frame channel) to an APC byte sequence
  injected into the byte stream at the correct offset. Existing
  Frame emission can stay for backward compat during Phase 5.
- Document the APC format in this PRD (already drafted §3.2).
- Daemon test: spawn a session, assert the APC byte appears in
  the byte archive at the grow-settle offset.

**Success criteria:** `hexdump` on the byte archive shows
`\x1b_k2so:grow_boundary:...\x07` at the right position.

### Phase 4 — Tauri-side Term per Kessel pane (~1 week)

- `src-tauri/src/commands/kessel_term.rs`: one `alacritty_terminal::Term`
  per pane, keyed by pane id. Owns a `Processor` + `LineMux` (for
  APC extraction; vte itself also handles APC but we want a
  pre-filter hook for k2so-namespace side effects).
- Tauri commands:
  - `kessel_attach(pane_id, session_id)` — open byte stream, pipe
    into Term.
  - `kessel_grid_snapshot(pane_id)` → `{grid, scrollback, cursor,
    viewport, modes}` — same shape as current `GridSnapshot`.
  - `kessel_resize(pane_id, cols, rows)` — calls `Term::resize`.
  - `kessel_write(pane_id, bytes)` — write to PTY via existing
    daemon endpoint.
  - `kessel_detach(pane_id)` — drop the Term.
- Snapshot push: emit `kessel:grid-snapshot` Tauri event on
  rAF-aligned cadence (not per-byte) so WebView doesn't drown in
  IPC.

**Success criteria:** Kessel pane renders from Term snapshots
instead of daemon Frames. Same visual output as today on
steady-state content.

### Phase 5 — Rewrite SessionStreamView to render from Term (~3-5 days)

- Delete `src/renderer/kessel/grid.ts` + `grid.test.ts`. The
  TypeScript TerminalGrid goes away.
- `SessionStreamView` subscribes to `kessel:grid-snapshot` events.
  Renders `visibleRows`, scrollback navigation, cursor from
  snapshot data — same DOM layout as today.
- Selection moves to content-space coords using `selection.ts`
  helpers. Overlay renders highlight rectangles via
  absolutely-positioned divs over the row spans. Selection
  survives scroll and resize (it's in Term coords).
- Find-in-scrollback as a bonus: `kessel_find(pane_id, pattern)`
  returns match positions; SessionStreamView renders them as an
  overlay similar to selection.

**Success criteria:** Reflow works on window resize. Selection
tracks content through scroll. Everything users currently do still
works.

### Phase 6 — Retire daemon-side Term (optional, ~1 day)

Post-Phase 5, Kessel no longer consumes the Frame stream — it
consumes bytes. The daemon's internal `alacritty_terminal::Term`
(currently gated by `track_alacritty_term: bool`) is only useful
for Frame consumers that want a derived grid. We can either:

- Keep daemon Term for Frame-stream consumers (mobile, etc.)
- Drop it; LineMux operates directly on bytes without a Term
  backing store. LineMux already does this — the Term is
  redundant for Frame production.

Decision deferred until Phase 5 ships and we see what Frame
consumers still need.

## 9. What this plan does not do

- **Does not migrate Claude to T1 stream-json.** That work is
  orthogonal; it lives on its own track in the original PRD.
  When T1 ships, narrower clients get unlimited semantic
  scrollback for free. Until then, grow-phase is the answer for
  wide-enough clients.
- **Does not change the daemon's role.** The daemon still owns
  the PTY, still runs LineMux, still routes AgentSignals, still
  supports the Frame broadcast. It just grows a second broadcast
  (bytes) and persists a second archive.
- **Does not touch the Awareness Bus.** Agent-to-agent signaling,
  cross-project messaging, scheduler-wake spawns, headless
  agent lifecycle — all unchanged. Kessel is a viewer, not a
  dependency for agent orchestration.

## 10. Success story

Someone opens K2SO 8 hours into a long Claude conversation they
started yesterday. Kessel pane mounts at their current window
size (say 125×59). Under the hood:

1. Tauri calls `kessel_attach(pane, session)`. A fresh Term is
   created at 125×59 with 5000 rows scrollback.
2. Daemon streams the byte archive from offset 0. Each chunk goes
   through the APC filter (strips `k2so:grow_boundary`, calls
   `sealGrowPhase` on the Term at the right moment) then through
   vte (drives Term grid and scrollback).
3. After replay, the Term contains: full conversation in
   scrollback, current Claude UI in live grid, cursor where it
   was when they last looked.
4. Live tail kicks in. New bytes arrive → Term updates → snapshot
   pushes to DOM.
5. User drags window wider to 150×72. `kessel_resize` →
   `Term::resize(150, 72)` → scrollback reflows at 150 cols
   automatically. DOM snapshot updates.
6. User scrolls up to read a message from earlier. Selection works
   across scrollback boundaries. Cmd+F opens find. Cmd+C copies
   cleanly.

No synthetic gymnastics. No ancestry tables. No preload race.
Just Alacritty doing what Alacritty does, driven by bytes the
daemon replays on demand.

## 11. Open questions

- **Snapshot cadence.** rAF-aligned on the Tauri side or
  delta-based (diff from last snapshot)? Delta-based is more
  work, lower bandwidth. Defer to Phase 4 benchmarks.
- **Scrollback persistence.** Alacritty's Term is in-memory only.
  If Kessel pane unmounts and re-mounts (tab hide/show), we
  re-replay from byte archive to rebuild. Cold reopen cost =
  archive size / throughput. Measure in Phase 4.
- **Write path.** Does `kessel_write` keep using the existing
  `/cli/terminal/write` HTTP, or get a dedicated byte-stream
  write channel for parity? The keystroke path is fine on HTTP
  today.

These are Phase-4/5 implementation details, not architecture
decisions. They get resolved as we build.

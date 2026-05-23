# Kessel Research Archive

**Status:** Historical record. Retired in 0.39.0.
**Captured:** 2026-05-23
**Author:** Rosson (direction), pod-leader (draft)
**Archive branch:** `archive/kessel-alpha-pre-0.39.0` — the only place
the Kessel v1 source still lives after 0.39.0.

This document is the durable account of the Kessel renderer: what
problem it tried to solve, why the v1 architecture didn't work, what
we shipped instead (the "active-viewer" workaround), and what a future
v2 should look like when someone returns to the multi-device-rendering
problem space. The reader should be able to finish this doc and decide
whether to resume work — or to start over — without needing to read
any git history.

If you're reading this because you want to bring Kessel back: skip to
[Layer 3](#layer-3-kessel-v2-the-right-answer).

---

## Problem statement (timeless)

A K2SO user can watch the same terminal session from multiple devices
simultaneously:

- A 14" MacBook (e.g. 120 cols × 36 rows)
- A 27" desktop monitor (e.g. 220 cols × 60 rows)
- An iPhone via Mobile Companion (e.g. 40 cols × 18 rows in portrait,
  rotating to landscape)
- A web viewer in a browser tab (variable)

Each viewer wants the session to render *at its own dimensions* so
that long lines wrap correctly, padding lines up with cell boundaries,
and the cursor lands where it should. The user expects to glance from
laptop to phone and see the same session, sized appropriately for
whichever screen they're looking at.

That's the founding promise. Everything below is about how hard that
promise turns out to be when the upstream producer is a TUI running
under a PTY.

---

## Layer 1: Kessel v1 (the alpha attempt)

### What it tried to do

The v1 design assumed the right architecture was a **shared byte
stream** from the daemon to N viewers. Each viewer would maintain its
own `alacritty_terminal::Term` at its own dimensions, run the same
PTY bytes through `vte::Processor::advance(&mut term, b)`, and project
the resulting grid to DOM.

The intuition: alacritty's `Term` natively reflows on `term.resize()`.
If every viewer has its own Term, every viewer reflows locally to its
own width. The daemon broadcasts the canonical byte stream; viewers
fan out and render.

### Data flow (v1)

```
                      [child process: claude (TUI)]
                                 │
                                 │ stdout bytes
                                 ▼
                  ┌──────────────────────────────────┐
                  │   kernel PTY (cols = X, fixed)   │
                  │   line discipline + wrap         │
                  └──────────────┬───────────────────┘
                                 │ bytes already
                                 │ laid out for X
                                 ▼
                       [daemon byte ring buffer]
                  /cli/sessions/bytes?session=…&from=N
                                 │
                  ┌──────────────┼──────────────┐
                  │              │              │
                  ▼              ▼              ▼
        ┌─────────────┐  ┌─────────────┐  ┌────────────┐
        │ Viewer A    │  │ Viewer B    │  │ Viewer C   │
        │ 220×60      │  │ 120×36      │  │ 40×18      │
        │ Term local  │  │ Term local  │  │ Term local │
        │ (DESKTOP)   │  │ (LAPTOP)    │  │ (PHONE)    │
        └─────────────┘  └─────────────┘  └────────────┘
              │                │                │
              └─── all three render the SAME bytes,
                   but the bytes were laid out for ONE width
                   (whichever the kernel PTY was sized at).
```

### What lived where (v1)

| File | Role |
|---|---|
| `crates/k2so-core/src/terminal/session_stream_pty.rs` | The Phase-2 PTY reader. Owns the read loop, drives bytes byte-by-byte through `vte::Processor::advance(&mut term, b)`, publishes to the daemon's byte ring. Defines `SessionStreamSession` (the v1 session handle). |
| `crates/k2so-daemon/src/session_map.rs` | The v1 registry: `agent_name → Arc<SessionStreamSession>`. Daemon-owned because the "inverse lookup" (write bytes into agent X's PTY) was a daemon concern, not a core/library one. |
| `crates/k2so-daemon/src/awareness_ws.rs:55-193` | The v1 HTTP handlers: `POST /cli/sessions/spawn` (lines 55-135) and `POST /cli/sessions/close` (lines 156-193). The Cmd+T → spawn → render path entered the daemon here. |
| `src-tauri/src/commands/kessel.rs` | The Tauri command bridge: cached daemon port/token, persistent `reqwest::blocking::Client`, one-IPC-hop spawn so the browser never paid fetch overhead. |
| `src-tauri/src/commands/kessel_term.rs` | Canvas Plan Phase 4 — a Tauri-side `alacritty_terminal::Term` per Kessel pane. The frontend stopped consuming Frame events; it asked this module for grid snapshots each rAF tick. Owned the APC filter that handled `\x1b_k2so:grow_boundary\x07` and (later) `\x1b_k2so:resize\x07`. |
| `src/renderer/kessel/KesselTerminal.tsx` | React tab-pane wrapper. Lifecycle: on mount → `kessel_spawn` IPC → mount `SessionStreamView` (or `SessionStreamViewTerm`) with the returned `sessionId`. |
| `src/renderer/kessel/HarnessLab.tsx` | A visual validation surface (Phase 4.5). Dropdown of preset commands (bash, zsh, claude, htop, vim), Spawn button, Kessel pane. Intentionally isolated from the tab/project system so screenshot tests could pin the renderer in isolation. |
| `src/renderer/kessel/SessionStreamView.tsx` | The original Frame-stream renderer (TerminalGrid → DOM spans). Superseded by `SessionStreamViewTerm.tsx`. |
| `src/renderer/kessel/SessionStreamViewTerm.tsx` | The byte-stream renderer that reads snapshots from the Tauri-side `Term` instance via `kessel_term_grid_snapshot`. |

### Why v1 didn't ship

The kernel does not give us what we wanted. Here is the irreducible
constraint, stated three ways so it sticks:

1. **Layout is computed before bytes reach the daemon.** When `claude`
   emits "move cursor to column 80" or "draw this box from col 4 to
   col 76", those positions were computed against the PTY's *current
   width*. The kernel's line discipline has already wrapped, padded,
   and committed those bytes to the master FD by the time the daemon
   reads them.

2. **A PTY has one size.** `ioctl(TIOCSWINSZ)` sets dimensions for
   *the child* — there is no per-subscriber PTY view. Two viewers at
   different widths cannot both get a "correctly-sized" byte stream
   from one PTY.

3. **Reflow works for flowing text, not for positioned UI.** Alacritty
   *can* reflow paragraphs on `term.resize()` because flowing text has
   no committed wrap points. It *cannot* reflow box-drawn TUIs (claude's
   message panes, htop's bars, vim's status line). The bytes that drew
   the box at col 80 still say col 80; the receiver's narrower Term
   either truncates or wraps into the next row, producing cascading
   redraw artifacts.

The first time we sat two viewers on the same Kessel session at
different sizes, the smaller viewer looked like a glitch reel: prompt
bars stacked two-high, claude's TUI banner duplicated and interleaved
with scrollback, padding chars consuming half the visible cells.

`.k2so/prds/kessel-resize-architecture-notes.md` is the in-the-trenches
log of us chasing paint races and wipe hacks before accepting that the
shape of the problem was wrong. The compressed lesson:

> **Don't try to re-render someone else's pre-rendered output.**
> Render from a source-of-truth that hasn't been laid out yet.

We confirmed this dead-end in `.k2so/prds/kessel-t1.md`:
> "A PTY has one size. The child process emits bytes laid out for that
> one width. Layout-positioned output cannot be 'reflowed' at the byte
> layer — only flowing text reflows via alacritty's natural wrap
> behavior. No per-subscriber reflow is possible when all subscribers
> share one byte pipeline."

That document also catalogs the secondary scar tissue: a grow-then-
shrink protocol, APC `k2so:grow_boundary` injection, a wipe hack that
broke workspace-return (`cbb8a30f` reverted), an APC `k2so:resize`
serialization scheme. Each was a heroic patch on the wrong premise.

---

## Layer 2: Active-viewer (the pragmatic workaround we shipped)

After v1 failed, we conceded the multi-device dream and shipped a
single-active-viewer model in 0.37.11 (A9 Phase 4 lineage). It
remains the production behavior as of 0.38.x.

### What it is

The daemon tracks one "active viewer" per session: the subscriber that
is currently foregrounded, focused, or last-resized. Only the active
viewer's `Resize` messages are honored. The PTY's `ioctl(TIOCSWINSZ)`
matches the active viewer's dimensions. Non-active viewers receive the
same byte stream but accept that it's laid out for someone else's
screen.

Most users only actively look at one device at a time. The phone in
your pocket can show garbled output while you work on the laptop; the
moment you pick the phone up and it claims active, the daemon
re-resizes the PTY to the phone's dimensions and a SIGWINCH-triggered
repaint cleans up the display.

### Data flow (v2 == active-viewer)

```
                      [child process: claude (TUI)]
                                 │
                                 │ stdout bytes
                                 ▼
              ┌────────────────────────────────────────┐
              │   kernel PTY (cols = ACTIVE_VIEWER.W,  │
              │              rows = ACTIVE_VIEWER.H)   │
              │   resized via SIGWINCH on claim swap   │
              └──────────────────┬─────────────────────┘
                                 │ bytes laid out
                                 │ for active viewer
                                 ▼
                       [daemon broadcast]
                                 │
                  ┌──────────────┼──────────────┐
                  ▼              ▼              ▼
        ┌─────────────┐  ┌─────────────┐  ┌────────────┐
        │ Viewer A    │  │ Viewer B    │  │ Viewer C   │
        │ 220×60      │  │ 120×36 ★    │  │ 40×18      │
        │ (jank)      │  │ (ACTIVE)    │  │ (jank)     │
        └─────────────┘  └─────────────┘  └────────────┘

  ★ ACTIVE viewer: PTY is sized for B. B sees crisp output.
  A and C see whatever shape the bytes were laid out in,
  which doesn't match their grids. They tolerate it until
  one of them claims active, at which point a SIGWINCH
  repaint refreshes the canvas.
```

### Where the workaround lives

The active-claim mechanism is part of the v2 (Alacritty_v2 /
`DaemonPtySession`) lineage, not Kessel v1. It is, however, the
practical answer to Kessel v1's original question. Pointers:

| File | Role |
|---|---|
| `crates/k2so-core/src/terminal/daemon_pty.rs:237-253` | `active_subscriber: AtomicU64` field on `DaemonPtySession`. Zero = no claim; non-zero = monotonically-increasing subscriber id. Documented inline. |
| `crates/k2so-daemon/src/sessions_grid_ws.rs:112-120` | The WS protocol's `SetActive { active: bool }` inbound message. Renderer sends `true` on focus, `false` on blur. |
| `crates/k2so-daemon/src/sessions_grid_ws.rs:122-128` | `NEXT_SUBSCRIBER_ID: AtomicU64` — monotonic id generator; each WS accept claims the next value. |
| `crates/k2so-daemon/src/sessions_grid_ws.rs:418-440` | The gating: incoming `Resize { cols, rows }` checks `session.active_subscriber.load()`. If `0` (no claim yet) or matches this subscriber's id, call `session.resize()`. Otherwise drop the resize with a `resize_ignored` log. |
| `crates/k2so-daemon/src/sessions_grid_ws.rs:441-479` | The claim/release: `SetActive { active: true }` stamps this subscriber's id into `active_subscriber`; `false` does a compare-exchange to clear only if we still hold the claim. |

The "first-resize-wins for sessions where no one ever sends SetActive"
fallback preserves single-viewer behavior for clients that don't
implement the protocol (older renderers, CLI tools).

### Why it's "good enough" today

- **One active user at a time** is the dominant pattern. Multi-device
  *simultaneous active* is rare.
- **SIGWINCH repaints clean up jank** when the active claim moves. The
  user sees a glitch for one frame and then a correct render.
- **The daemon stays simple.** No per-viewer PTY, no per-viewer
  rendering server, no schema for "this viewer wants 120×36 of the
  same byte stream."
- **It's lock-free.** `AtomicU64` for the active claim; no contention
  visible at the per-frame level since claims fire on focus changes,
  not on every keystroke.

### What it doesn't solve

- A user actually watching from two devices at once (laptop in front,
  phone propped up) sees jank on the device that isn't active.
- A mobile companion sitting idle in a pocket displays garbage if the
  user wakes the phone screen briefly — until the mobile claims active
  and the SIGWINCH-repaint settles.
- Brief flashes of wrong-sized content during claim handoff.

For the use cases we care about today (one user, one active device,
phone-as-glance-not-control), this is acceptable. For the use cases
the original Kessel pitch promised (truly simultaneous multi-device
rendering), it is not.

---

## Layer 3: Kessel v2 (the right answer)

This is the design we'd build *if and when* the problem space matters
enough to revisit. It is not on the roadmap as of 0.39.0.

### The pivot

Render from claude's structured **JSONL session stream** (turns, tool
calls, content blocks) instead of the kernel's pre-laid-out byte
stream. The JSONL is structured, pre-layout data; each viewer's
renderer lays it out natively at its own dimensions.

This is the same conceptual move the `kessel-t1.md` PRD already
sketched ("T1 = consume the harness's semantic event stream"), with
the explicit framing: **don't fight the kernel; bypass it.**

### Data flow (v2)

```
                  [child process: claude --output-format stream-json]
                                  │
                                  │ stdout: NDJSON events
                                  │ (system init, user, assistant,
                                  │  tool_call, tool_result, result)
                                  ▼
                      ┌──────────────────────────┐
                      │ harness adapter          │
                      │ (claude_code.rs)         │
                      │ JSONL → semantic Frames  │
                      └──────────────┬───────────┘
                                     │ Frames
                                     ▼
                          [session::registry]
                          (broadcast channel)
                                     │
                  ┌──────────────────┼──────────────────┐
                  ▼                  ▼                  ▼
        ┌──────────────┐   ┌──────────────┐   ┌──────────────┐
        │ Viewer A     │   │ Viewer B     │   │ Viewer C     │
        │ 220×60       │   │ 120×36       │   │ 40×18        │
        │ React DOM    │   │ React DOM    │   │ React Native │
        │ <MessageList>│   │ <MessageList>│   │ <MessageList>│
        │ <ToolCard>   │   │ <ToolCard>   │   │ <ToolCard>   │
        └──────────────┘   └──────────────┘   └──────────────┘
              ▲                  ▲                  ▲
              │  Each viewer renders semantic events
              │  natively at its own dimensions.
              │  No shared layout. No PTY in the loop.
              │  Reflow is trivial because nothing was
              │  laid out upstream.
```

### Why this is fundamentally different from v1

- **The kernel never sees this path.** No PTY, no `ioctl(TIOCSWINSZ)`,
  no line discipline, no committed wrap points. The daemon controls
  the broadcast dimensions completely because there are no dimensions
  to control upstream.
- **The TUI's own layout is bypassed.** Claude's terminal UI rendering
  (the box-drawn message bubbles, the prompt input bar, the status
  line) is built by claude *for a terminal*. We are not consuming that
  output; we are consuming the conversation data the TUI was rendering
  *from*. Our renderer builds its own chat-like UI from that data.
- **Per-viewer rendering is trivial.** Each viewer's React tree owns
  its layout. There's no "what width was this rendered for" — there's
  just a Message component, a ToolCall component, a Thinking block.

### What's still needed to build v2

| Piece | Status |
|---|---|
| Harness adapter trait + claude adapter | Sketched in `.k2so/prds/kessel-t1.md`. Not implemented. |
| Normalized Frame vocabulary additions (`Frame::Message`, `Frame::ToolCall`, `Frame::ToolResult`, `Frame::Thinking`, `Frame::PlanUpdate`, `Frame::FileEdit`, `Frame::SystemInit`, `Frame::TurnBoundary`) | Sketched. Not built. |
| Chat-style renderer component (`<MessageBubble>`, `<ToolCallCard>`, `<ThinkingBlock>`, `<PlanTracker>`, `<FileEditPreview>`) | Sketched. Not built. |
| Settings toggle: "PTY viewer (current)" vs "Structured viewer (Kessel v2)" — per-session, per-workspace, or global | Not designed. |
| Schema pinning for claude's JSONL output | Not addressed. Claude changes their schema sometimes; we'd need to version-pin and gracefully degrade on unknown fields. |
| Path for non-claude commands | The PTY path stays as the default. The structured path is claude-specific (and gemini, codex, cursor, goose, pi-mono via per-tool adapters — see `kessel-t1.md` for the full catalog). Shells, vim, htop, raw `bash` continue to use Alacritty_v2. |

### Caveats / risks

- **JSONL is claude-specific.** The structured-viewer dream doesn't
  help with raw shells (`bash`, `zsh`), editors (`vim`, `nano`),
  monitoring tools (`htop`, `btop`), or any other TUI that doesn't
  emit a structured event stream. These remain on the active-viewer
  workaround indefinitely.
- **Schema drift.** Claude (and the other CLI LLM tools) can change
  their JSONL schemas without notice. The adapter layer needs versioning
  and conservative parsing — fall back to `Text` Frames on unknown
  variants, never crash the session.
- **Tool catalog maintenance.** Each new T1-capable tool needs its own
  adapter. `kessel-t1.md` lists six tools confirmed at the time of
  that PRD; the landscape will have shifted by the time anyone reads
  this.
- **Mobile Companion alignment.** The companion already consumes some
  form of structured data via the awareness bus. Kessel v2's Frame
  vocabulary might naturally dovetail with what the companion needs,
  or it might diverge in ways that double the work. Worth scoping that
  intersection before committing to a vocabulary.
- **The PTY path doesn't go away.** Kessel v2 is *additive*. It exists
  alongside Alacritty_v2 for tools that don't have a JSONL mode. The
  product surface adds a mode toggle, not a replacement.

---

## What we learned

Stated as principles, in order of how often each one bit us:

1. **Don't re-render someone else's pre-rendered output.** If the
   producer has already committed bytes to a width, you cannot un-bake
   them. Find the pre-layout source or accept that you only have one
   width.

2. **A PTY has one size.** This is a kernel invariant, not a K2SO
   limitation. Anything that wants multi-size views from a single PTY
   is fighting the kernel.

3. **Reflow works for flowing text only.** Alacritty's reflow is
   excellent for paragraphs. It is helpless against positioned TUIs.
   Most modern CLI tools (claude, codex, gemini, htop) are positioned
   TUIs.

4. **APC injection is a serialization tool, not a fix.** When we
   injected `\x1b_k2so:grow_boundary\x07` and later `\x1b_k2so:resize\x07`
   into the byte stream, we were *serializing* resize events with the
   bytes around them so the receiver could process them in order. That
   was correct engineering. It just wasn't solving the right problem.

5. **Workarounds that work for 95% of users are worth shipping.** The
   active-viewer model is, frankly, fine for almost everybody. The
   right move was to ship it as the production path and move on, not
   to hold the feature for the perfect solution. The lesson is to
   recognize the 95% solution *earlier* — we spent a lot of cycles on
   wipe hacks and grow-shrink protocols before accepting it.

6. **Architecture archives belong in markdown, not git history.** This
   document exists because three years from now nobody will know which
   commit had the wipe revert, which PRD captured the JSON pivot, or
   why the byte-ring code was deleted. They will read this file.

---

## Reading list (when you resume)

The minimum context to bring Kessel back:

1. **This document.** Establishes the problem space and the dead ends.
2. **`.k2so/prds/kessel-t1.md`.** The JSONL-pivot design sketch — the
   most thought-out version of what v2 should be.
3. **`.k2so/prds/kessel-resize-architecture-notes.md`.** The
   in-the-trenches log of v1's death. Read this so you don't re-do
   the wipe-hack chase.
4. **`.k2so/prds/kessel-instant-everywhere.md`.** The "ride quality"
   PRD — what good UX looks like in this space (instant workspace
   switches, paint decoupled from terminal creation, etc.). Still
   relevant as design north star.
5. **`.k2so/prds/canvas-plan.md`.** The Phase-4 architecture doc that
   established the Tauri-side `Term` per pane. Useful context for
   understanding what the v1 code was structured around.
6. **`.k2so/prds/a9-daemon-headless-session-unification.md`.** Explains
   how Alacritty_v2 became the production path and `session_map`
   became legacy-only. The active-viewer mechanism is part of this
   lineage.

To resurrect the v1 source for reference:

```bash
git checkout archive/kessel-alpha-pre-0.39.0 -- \
  src/renderer/kessel/ \
  src-tauri/src/commands/kessel.rs \
  src-tauri/src/commands/kessel_term.rs \
  crates/k2so-core/src/terminal/session_stream_pty.rs \
  crates/k2so-daemon/src/session_map.rs
```

(The `awareness_ws.rs` handlers were removed by surgical edit; check
the archive branch's full file rather than cherry-picking.)

---

## Decision record

| Date | Decision | Outcome |
|---|---|---|
| 2026-04-22 | Ship 0.34.1 with Kessel v1 (byte-stream + Tauri Term) as alpha | Shipped. |
| 2026-04-22→2026-04-24 | Chase paint-race / resize bugs (`cbb8a30f` wipe + revert) | Workaround stacking became unsustainable. |
| 2026-04-24 | Capture the JSONL pivot in `kessel-t1.md` | Acknowledged v1 architecture was wrong. |
| 2026-05-11 | A9 Phase 4 ships active-viewer gating in Alacritty_v2 | Production multi-device behavior settled. |
| 2026-05-23 | Retire Kessel v1; archive source to `archive/kessel-alpha-pre-0.39.0`; write this document | The branch lives. The code is out of `main`. Future engineers know where to look. |

---

*Written so the next person who picks this up doesn't have to read git
history. If you're that person: good luck. The problem space is real,
the v2 pivot is correct, and the active-viewer workaround buys you
plenty of time to do it right.*

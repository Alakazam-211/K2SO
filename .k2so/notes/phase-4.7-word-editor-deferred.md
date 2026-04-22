# Phase 4.7 — Word-Editor Tooling (DEFERRED)

**Branch:** TBD (off `feat/session-stream` or successor post-4.6)
**Status:** deferred — captured now so Phase 4.6 doesn't foreclose it.
**Origin:** Rosson's screenshot of a prior brainstorm, titled
*"Word-editor tools via Frame primitive"*, surfaced 2026-04-21.

---

## Why we're writing this down now

Rosson's direction:

> *"Some of the features we may find valuable in the future is the
> ability to allow users to click their cursor into a line at a
> certain position and highlight some text, start typing, and
> replace that text like a normal word editor. We'll save this as
> 4.7 and likely defer it for now, but we don't want to build
> anything that could break this potential improvement."*

Two jobs for this doc:
1. **Preserve the vision.** Concrete deliverables so when we're
   ready to commit to 4.7, the research is done.
2. **Bind Phase 4.6.** Every deliverable here is a set of
   constraints on what 4.6 is allowed to do.

---

## The thesis — why this is possible at all

Since we own every cell in TypeScript and know its semantic
(prompt? tool output? model response?) via `Frame::SemanticEvent`,
we can build things alacritty-DOM can't. The precondition is
owning the state machine in TypeScript — which is exactly what
Phase 4.5's `TerminalGrid` does. Everything in this phase is a
layer on top.

Traditional terminals are write-only from the user's perspective:
type a command, read the output. Our Session Stream gives us a
structured event stream where every frame carries intent. That
turns the terminal pane into something closer to a document:
cells are addressable, semantic spans are identifiable, the full
history is replayable, and the user can reach into any of it.

---

## The seven features (verbatim from the brainstorm, expanded)

### F1 — Highlight + ask

**Vision:** Select text → right-click → "Ask Claude about this" →
wires into the signal primitive.

**Mechanic:**
- Browser-native selection already works (we render DOM spans).
- Right-click menu is one element above the pane.
- "Ask Claude" constructs an `AgentSignal` with
  `SignalKind::msg { text: "About this output: <selected text>" }`
  and posts it to whichever agent owns the session.
- Rides the existing awareness bus — no new daemon routes needed.

**Precondition from 4.6:** selection must survive all renders. No
replacing spans with canvas, no `user-select: none`, no style
collapse that obliterates character boundaries.

**Dependency:** F2 input region awareness.

---

### F2 — Replace-in-place

**Vision:** Edit a tool output cell, persist via Awareness
metadata, feed the edit into the next step.

**Mechanic:**
- On click into a cell inside a tool-output semantic region,
  overlay an editable `<input>` (or a `contentEditable` span) at
  that cell's coordinates.
- Edit is an *annotation*, not a rewrite of the PTY stream — the
  original bytes stay on the Frame archive. The edit is stored as
  an `AwarenessEntry` tagged with the `(sessionId, row, col, len)`
  and the new text.
- On re-render, the TerminalGrid consults the annotation layer
  and overlays the edit visually. The next agent turn receives
  both the original output and the edit in its prompt context.

**Precondition from 4.6:** row/col identity must be stable across
rAF cycles. Damage tracking (D3) must not clobber annotation
overlays.

**Dependency:** requires SemanticEvent to tag row ranges with
their harness + kind (prompt vs. tool output vs. model response).

---

### F3 — Jump-to-source

**Vision:** File paths in output become clickable links.

**Mechanic:**
- Scanner runs over every newly-committed `Line` from LineMux,
  extracting `path:line:col` patterns (same regex
  AlacrittyTerminalView uses — there's a `terminalLinkDetector.ts`
  we can port).
- Matches become spans with an `onClick` that dispatches a
  `file:open` Tauri IPC (already exists for the editor pane).
- Could run on the Rust side (cheaper, one pass per commit) and
  surface as a `SemanticKind::Custom { kind: "file_link" }` event
  with a structured payload. Or TS-side on each render — cheaper
  to iterate UX on.

**Precondition from 4.6:** per-span click targeting must not be
broken by D3's row memoization. Easiest way to preserve this: if a
row contains any cell with `style.hyperlink !== null` (or any
future `style.link`), memoization bypasses for that row.

**Overlaps with 4.6 D11 (OSC 8 hyperlinks):** F3 is the *detect*
case; D11 is the *honor explicit* case. Share the link rendering
layer between them.

---

### F4 — Timeline scrubber (VCR)

**Vision:** The Frame archive (NDJSON we already write per
session) is a full recording. Build a VCR — scrub to any point,
branch from there.

**Mechanic:**
- Archive writer already exists (Phase 3 commit `ca834a07`,
  `session::archive` task).
- Scrubber UI: horizontal timeline below the pane. Each frame has
  a timestamp; the slider maps time → frame index.
- "Play back" rebuilds `TerminalGrid` state by replaying frames
  from index 0 to the cursor. For large sessions this needs
  keyframing: every N frames, snapshot the grid; scrubbing jumps
  to nearest keyframe and forward-replays.
- "Branch from here": reads the frames up to the scrub point,
  spawns a new session with those frames as starting context.
  The Agent Signal primitive can carry the branch context.

**Precondition from 4.6:** D1 synchronized output must not drop
individual frames in the archive. The renderer can coalesce
observable state changes, but the archive writer sees every
frame. This is already the case today (archive subscribes at
the entry level, not at the grid level); just don't regress it.

**Precondition from 4.6:** D4 WS frame batching must not lose
frames. Batching is about delivery cadence, not content.

---

### F5 — Annotations

**Vision:** Add a note to a line, persist cross-session, re-render
on next load.

**Mechanic:**
- Annotations stored in SQLite `session_annotations` table:
  `(sessionId, row_in_archive_sequence, text, author, created_at)`.
- Row index refers to a *line sequence number* (we already have
  `SeqnoGen` on the Line type) — stable across replays.
- Renderer: for each committed line, consult the annotation layer
  and overlay a gutter marker + hover tooltip.

**Precondition from 4.6:** `Line.sequence_no` must remain stable
and monotonic. This is a Phase 1 invariant already — don't break
it while adding new frame variants.

---

### F6 — Rich prompt editor

**Vision:** Input region gets markdown formatting, slash commands,
attachments, drag-and-drop.

**Mechanic:**
- Detect the input-line region by watching `SemanticKind::Message`
  events + `restingCursor` position. The lines that are "the user's
  current input" are identifiable in harnesses like Claude Code
  (box recognizer already surfaces prompt region in C6).
- Replace those rows with a CodeMirror instance (already used
  elsewhere in the app). CodeMirror handles markdown, completions,
  attachments.
- On submit, CodeMirror's text is serialized to raw PTY bytes and
  sent via `/cli/terminal/write`.
- This is the most invasive feature. It partially decouples the
  pane from "render the PTY output" toward "render the document
  the PTY represents."

**Precondition from 4.6:** 
- `D5` (APP_CURSOR) must not assume the whole pane is a PTY input
  — when the input region is a CodeMirror instance, arrow keys
  belong to CodeMirror first.
- `D8` (mouse reporting) must not capture mouse events destined
  for the CodeMirror region. Mouse dispatch has to be
  *region-aware*.
- SessionStreamView's keydown handler becomes one of several
  input handlers, gated by "is the cursor inside a PTY region or
  an overlay region?"

**Recommendation:** design D8's mouse-event dispatcher as
*pluggable region dispatchers*, not a single "forward all mouse
to PTY" toggle. Same for D5's keyboard handler.

---

### F7 — Diff overlay

**Vision:** Compare two session runs cell-by-cell.

**Mechanic:**
- Load two archives, replay both to a common point (or to their
  respective ends).
- Align rows by sequence-no / semantic region.
- Render a side-by-side or interleaved view with per-cell red/
  green highlighting for changes. The cell-level diff is the
  whole point — text diff tools work at line or word level; we
  can do character level because we own every cell.
- Exceptionally useful for debugging agent runs that should have
  been deterministic.

**Precondition from 4.6:** the `Cell` shape must not lose
identity. Don't flatten cells into strings, don't collapse runs
into opaque objects. Cells stay `{ char: string, style: Style | null }`.

---

## Binding constraints on Phase 4.6

The above features share seven pre-conditions. Every 4.6
deliverable must honor them. Flag each constraint in the 4.6
deliverable's commit message so future archaeology is easy.

### C1 — Row/col identity is permanent

Every visible cell lives at a stable `(rowIdx, colIdx)` for the
lifetime of one render cycle. `cellMetrics.width × colIdx` + the
pane's left-edge must resolve to the cell's on-screen x position,
exactly. No sub-pixel drift, no variable-width fonts.

**Affected 4.6 items:**
- D2 (dirty flag) — fine.
- D3 (line damage) — memoize by row identity, not by a content
  hash that could accidentally collide.
- D10 (palette) — palette resolution happens inside `styleToCss`;
  the coordinate system is untouched.

### C2 — Per-cell click targeting

`renderRow` currently coalesces adjacent same-style cells into
one `<span>`. This is fine for rendering but means `event.target`
gives us the span, not the cell. F1 / F2 / F3 all need cell
coordinates from a click.

**Resolution:** compute col from `(event.clientX - pane.left) /
cellMetrics.width`. Do NOT stop coalescing — that's a perf
regression. Instead, expose a `coordsFromMouseEvent(e)` helper on
SessionStreamView that does the arithmetic.

**Affected 4.6 items:**
- D8 (mouse reporting) — the mouse encoder already needs
  row/col → this helper is the natural home. Build D8 using
  `coordsFromMouseEvent` and F1 inherits it.

### C3 — Frame archive is lossless

Every single `Frame` that LineMux emits reaches the archive
writer unchanged. No de-duping, no coalescing, no filtering in
the archive path. The renderer can coalesce *rendering*; the
archive sees truth.

**Affected 4.6 items:**
- D1 (synchronized output) — buffer is on the renderer side only.
  The `SessionEntry` broadcast channel (upstream of both renderer
  and archive) must see every frame. Today this is true because
  the PTY reader's `dual-emit` path fans out before the buffer;
  don't change that.
- D4 (WS frame batching) — batch at the client/network boundary,
  not upstream of the entry broadcast.

### C4 — `SemanticEvent` frames preserved end-to-end

F1, F2, F6 all need to know "what semantic region am I in" —
prompt, tool output, model response. LineMux emits
`SemanticEvent` today via the recognizer pipeline; the grid
ignores them. That's intentional for 4.5 (pure display) but 4.7
needs them on the TypeScript side.

**Constraint:** don't drop `SemanticEvent` frames in any 4.6
deliverable. TerminalGrid can continue ignoring them at apply
time, but they must remain visible in the snapshot or via a
subscriber hook.

**Affected 4.6 items:**
- D2 (dirty flag) — SemanticEvent frames should set dirty so
  subscribers see them on the next rAF. Or expose a separate
  `semanticEvents` array in the snapshot.
- D4 (WS batching) — keep order intact; SemanticEvent after a
  Text frame must stay after it in the batch.

### C5 — `Line.sequence_no` is monotonic and stable

F5 annotations pin to line sequence numbers. The ordering must
survive PTY reconnects, session replays, daemon restarts.

**Affected 4.6 items:**
- None directly — this is a Phase 1 invariant. Just don't break
  it while adding frame variants.

### C6 — Cell shape stays `{ char, style }`

F7 diff overlay needs cells comparable cell-by-cell. If we ever
add something like `{ char, style, metadata: ... }` we must make
`metadata` optional and default-null so two cells from different
sessions compare equal when their semantic content matches.

**Affected 4.6 items:**
- D11 (OSC 8 hyperlinks) — adding `hyperlink: Option<String>` to
  `Style` is fine. Style already has optional fields. Comparison
  logic should stay `style === style || (style_a.fg === style_b.fg
  && ...)` — don't introduce a deep comparator that treats `null`
  ≠ `undefined`.

### C7 — Region-aware input dispatch

Phase 4.6's input deliverables (D5, D8) must be implemented such
that *adding* a region-aware layer later is additive, not a
rewrite. Concretely:

- D5: keyboard handler should read an optional
  `keyboardRegionDispatcher` prop; default is the current
  all-to-PTY behavior.
- D8: mouse handler should read an optional
  `mouseRegionDispatcher` prop; default is all-to-PTY if mouse
  mode is on, otherwise scrollback handling.
- F6's rich prompt editor becomes a dispatcher implementation.
  4.7 can then add it without touching D5/D8's code.

**Affected 4.6 items:**
- D5 (APP_CURSOR) — add the prop but don't wire a dispatcher yet.
- D8 (mouse reporting) — build around the dispatcher shape from
  day one.

### C8 — Alt-screen is a hard kill switch for 4.7 features

**Rule:** when `TerminalGrid.modes.altScreen === true`, EVERY 4.7
feature stands down. No exceptions. Vim, htop, less, lazygit,
neovim, claude --fullscreen, and every other TUI that owns the
whole buffer retains full control of input and display.

**Why:** word-editor ergonomics (highlight-and-ask, replace-in-
place, path-to-link transformation) make sense for AI-agent
session output where Frame::SemanticEvent tags recognizable
regions. They do NOT make sense inside a TUI where every
keystroke and click is the TUI's to interpret. Terminal purists
who drop into vim from a shell MUST experience vim exactly the
way they would in iTerm2 or Terminal.app. Any 4.7 feature firing
inside vim is a product bug, full stop.

**Implementation shape (for when 4.7 starts):** every 4.7 dispatch
hook — menu, click, hover, key — checks `modes.altScreen` as the
first gate. Alt-screen true → short-circuit, pass through to the
existing PTY path unchanged.

**Affected 4.6 items:** none directly (4.6 doesn't add 4.7
features). But D8's mouse dispatcher should structure itself
such that "alt-screen on → forward to TUI" is the first branch,
not a later filter. Makes the 4.7 add-on trivially correct.

---

## Toggle model (for when 4.7 starts)

A global setting, three-tier:

```
kessel.wordEditor: 'auto' | 'off' | 'all'    // default 'auto'
```

- **`auto` (default)** — F1 / F2 / F3 / F5 activate only when
  both: (a) `modes.altScreen === false`, AND (b) the target row
  is inside a SemanticEvent region from a recognized harness.
  F4 + F7 always available (they're separate UI surfaces, not
  inside the pane). F6 opt-in via a sub-setting.
- **`off`** — pure terminal. No menus, no intercepted clicks, no
  editable overlays, no link transformation. For dotfile crowd
  and vim purists who want byte-for-byte iTerm2 parity.
- **`all`** — F6 everywhere, maximum feature surface. For power
  users and testing.

The C8 alt-screen rule overrides all three settings. Even with
`kessel.wordEditor: 'all'`, vim gets pure terminal behavior
because alt-screen is on. This is intentional.

**Per-feature risk reminder:**

| Feature | Risk | Activation scope in `auto` |
|---|---|---|
| F1 Highlight + ask | Low | !altScreen, any row |
| F2 Replace-in-place | Medium | !altScreen + SemanticEvent region only |
| F3 Jump-to-source | Low-med | !altScreen, any row |
| F4 Timeline scrubber | None | Always (separate UI) |
| F5 Annotations | Low | Always (gutter overlay, not input-capturing) |
| F6 Rich prompt editor | **High** | Opt-in via sub-setting even in `auto` |
| F7 Diff overlay | None | Always (separate view) |

---

## Estimated scope (for context, not commitment)

If someone started 4.7 today, rough order-of-magnitude:

| Feature | Commits | Why |
|---|---|---|
| F1 Highlight + ask | 2-3 | Tiny menu + signal wire |
| F2 Replace-in-place | 8-10 | Annotation layer, overlay renderer, context feed |
| F3 Jump-to-source | 2-3 | Port link detector, wire onClick |
| F4 Timeline scrubber | 6-8 | Keyframing + UI |
| F5 Annotations | 3-4 | SQLite + gutter renderer |
| F6 Rich prompt editor | 10-15 | CodeMirror integration, region dispatch, byte serialization |
| F7 Diff overlay | 5-7 | Align + render |

Total: ~40 commits. Real 2-3 month phase for one engineer.

---

## Out of scope even for 4.7

- **Full IDE-style refactoring in the terminal.** No jump-to-def,
  no rename, no go-to-test. That's IDE territory.
- **Undo across agent turns.** The agent's output is canonical;
  we annotate/overlay, we don't rewrite history.
- **Collaborative editing.** Not building a Google Docs.

---

## Open questions (for when we un-defer)

1. Does F2 replace-in-place feed the edit into the *next* agent
   turn's context, or into the PTY directly as a simulated
   correction? (The image says "feed the edit into the next step" —
   ambiguous.)
2. For F4 timeline scrubber, how much of the archive do we
   preload vs. stream? Large sessions could be MBs of NDJSON.
3. F6 rich prompt editor raises IME support, accessibility, and
   focus-management questions that don't fit the terminal model.
   Might be worth scoping to "plain textarea with markdown
   preview" for MVP.
4. F7 diff overlay: do we diff the Frame stream, the rendered
   grid state, or the Line stream? Each gives a different answer.

---

## One more thing

> *"The precondition for all of this is owning the state machine
> in TypeScript, which is exactly what I3 builds. Good instinct."*

We already own it. Phase 4.6 ends with us owning it better.
Phase 4.7 is what we spend that ownership on.

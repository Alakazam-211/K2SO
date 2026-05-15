# K2SO 0.34.2 — Full conversation on resume

Kessel now shows the **complete conversation in scrollback** when
you resume a Claude Code session, instead of just the most recent
few lines. The daemon captures Claude's full cold-start paint at
an oversized canvas, then cleanly seals that content into
scrollback before handing the pane off at your real window size.

## Highlights

### Full conversation history on `claude --resume`

Previously, resuming a session painted Claude's current UI at 24
rows and nothing else — all the prior conversation lived in
Claude's state but never made it into the terminal. Now:

- Daemon spawns every session at an oversized canvas so Claude
  paints its full conversation context.
- A `grow_boundary` marker cleanly separates the grow-phase paint
  from the live session — no garbled "two renderings colliding"
  artifacts.
- Client pushes all grow-phase content into scrollback before
  Claude's post-SIGWINCH repaint can clobber it. Scroll up — the
  whole conversation is there.

Works for fresh spawns, resumes, heartbeat-triggered wakes, and
every subscriber that attaches after spawn time. No special path
for the first viewer vs the hundredth.

### Scroll fixes

- **Bottom row staleness is gone.** Scrolling up into scrollback
  then back down now re-renders the revealed rows correctly
  instead of keeping the pre-scroll content. The row-memo
  comparator now checks reference identity, not just the damage
  flag.
- **Cursor follows the viewport.** When you scroll into scrollback,
  the cursor stays visible at its translated position instead of
  disappearing. It only hides when scrolled past the cursor row.

### Settle behavior fix (resume reliability)

Dropped the bracketed-paste fast-settle path that was firing
during Claude's cold-start — before Claude had read the saved
conversation from disk. Now settle is idle-only (400ms quiet
after the first frame) plus a 3s ceiling. Adds ~300ms to fresh-
launch spawn time in exchange for correct resume capture. A
worthwhile trade — the resume path is the whole point.

### Observability

Daemon logs now surface the ring state end-to-end:

- `grow-shrink: session X settled via Y — ring before shrink:
  frames=N text=T mode=M bytes~=B subscribers=S`
- `emitted grow_boundary frame (target=CxR, grow_rows=500)`
- `subscriber for X will drain replay: frames=N text=T
  text_bytes=B mode=M grow_boundary=Y/N`

Enough to diagnose "did the ring have the data?" vs "did the
client render it?" independently. Visible in
`~/.k2so/daemon.stderr.log`.

## Architecture notes

Full design of the next steps is captured in
`.k2so/prds/canvas-plan.md`. 0.34.2 ships **Phase 1** of that
plan (the seam fix); Phases 2-5 add a byte-stream subscription
tier and a Tauri-local `alacritty_terminal::Term` per Kessel
pane so reflow, selection, and scrollback become first-class
instead of handwritten. Not in this release; tracked separately.

## Bug fixes

- `sealGrowPhase` — fixes the specific case where the bottom
  24 rows of a grown conversation were being wiped by Claude's
  post-SIGWINCH ClearScreen. Full conversation now lands in
  scrollback intact.
- Safety fallback on older daemons: if the client doesn't see a
  `grow_boundary` marker within 3 s of the subscribe ack, it
  measures the real container dims and falls back cleanly.
- DOM selection is cleared on scroll so highlights don't desync
  from their content. Proper follows-content behavior will land
  in Phase 5.

## Known limitations

- **Scrollback doesn't reflow on window resize.** The grid grows
  and shrinks correctly at the cell level, but scrollback rows
  retain their capture-time width. The Canvas Plan (Phase 4-5)
  solves this by moving each Kessel pane onto a local
  `alacritty_terminal::Term`, which reflows natively.
- **Selection drops on scroll rather than following content.** A
  proper content-space selection overlay is in Phase 5.
- For CUP-based TUIs (Claude, htop), scrollback is a moment-
  capture at spawn time, not a continuous feed — Claude's post-
  boundary UI updates don't add to scrollback. The PRD's T1
  (stream-json) path is the long-term answer for unlimited
  semantic scrollback.

## Install

Same as prior releases — download the signed, notarized DMG from
the GitHub release page. In-app auto-updater will pick up 0.34.2
automatically if you're on 0.34.0 or later.

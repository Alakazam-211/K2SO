# Phase 4.6 — Kessel Parity & Polish

**Branch:** `feat/session-stream` (continuing off Phase 4.5)
**Targets:** bugs + alacritty-parity gaps Rosson surfaced while
daily-driving the Kessel pane post-4.5.
**Mandate from Rosson:** *"We have the time and the money. Let's
take everything we've learned and turn it into a Phase 4.6."*

---

## Why 4.6 exists

Phase 4.5 shipped the Kessel renderer end-to-end. Rosson then spent
real time running `claude` / bash / editors inside the pane and
surfaced a series of issues — none of them blockers, all of them
"this is how a real terminal should feel" gaps. Every item below
traces to a specific observation during the 2026-04-21 testing
session or the alacritty source study we did afterward.

The phase is organized as two tracks:

1. **Reactive track** — bugs + UX gaps surfaced by Rosson.
2. **Alacritty-parity track** — SOTA patterns the study surfaced
   that we should earn before telling anyone "ship it."

Both tracks sit on top of the same `Frame::ModeChange` /
`TerminalGrid.modes` rail that landed in the opening of 4.6.

---

## What's already shipped in 4.6 (the opening batch)

Six commits on `feat/session-stream`, all in response to Rosson's
in-session feedback + the alacritty study:

| Commit | Deliverable | Observation that drove it |
|---|---|---|
| `e2ee0a8c` | Remove cursor blink | "Ditch the blinking cursor, solid cursor, no blinking." |
| `21697093` | Scrollback viewport + wheel nav | "Can't scroll up once the messages are above my position." |
| `2f4def90` | DECTCEM `?25` cursor visibility | Attacks cursor-jump at its source: Claude hides-then-shows across each repaint. |
| `5ccd7c68` | Bracketed paste `?2004` | Multi-line pastes into Claude auto-submit mid-paste. |
| `f2fa6d7d` | Alt screen `?1049` / `?47` | vim/less/htop/claude-full-screen would otherwise smear the shell scrollback. |
| `3fd05edd` | `naturalTextEditingSequence` wiring | "Cmd+Backspace should delete the current line." (also Cmd+←/→, Option+Back, etc.) |

**Tests baseline at start of 4.6:** 29 LineMux + 26 grid.
**Tests after opening batch:** 36 LineMux + 29 grid. Typecheck clean.

---

## Architectural rail the opening laid down

One idea worth calling out because the rest of the phase rides on
it: `Frame::ModeChange { mode: ModeKind, on: bool }`.

`ModeKind` is `#[non_exhaustive]` with `BracketedPaste` + `AltScreen`
today. Every new private-mode deliverable below adds one variant,
wires the CSI dispatcher, and updates the TS mirror. The consumer
side is a single `switch` in `TerminalGrid.handleModeChange` + (for
modes that affect input) a snapshot read in `SessionStreamView`.

The same rail absorbs all the remaining `DECSET ?N` items without
further churn.

---

## Deliverables — remaining scope of 4.6

Ordered roughly by impact × difficulty (best ratio first). Each
entry has:
- **Cite:** the observation or principle it traces to.
- **Implementation sketch:** enough detail to start without more
  research.
- **Tests:** what has to be green to ship.
- **Estimate:** commits + rough size.
- **Risk:** what could go wrong.

### D1 — Synchronized Output (`DECSET ?2026`)

⭐ **Highest resilience win remaining.** Claude and other modern
TUIs wrap large repaints in `?2026 h` … `?2026 l`. Honoring this
eliminates the last 10% of cursor jitter and partial-repaint flashes
that our settle window is currently papering over.

**Cite:** Rosson's original "cursor is bouncing around" thread +
alacritty's `SyncUpdate` handling in `alacritty_terminal/src/vte.rs`.

**Implementation sketch:**
- Add `ModeKind::SynchronizedOutput`.
- LineMux: dispatch `?2026 h` / `?2026 l` as `ModeChange`.
- `TerminalGrid`: when `on`, stash incoming `Frame`s into a pending
  buffer instead of mutating state. When `off`, drain the buffer
  into a single atomic apply + one snapshot change. The consumer
  sees one render per repaint instead of ~15 intermediate ones.
- Watchdog: safety ceiling on the buffer (alacritty uses 150ms or
  a config cap). If exceeded, force-flush and log — a buggy TUI
  that never emits the close must not wedge the pane.
- Renderer: no changes needed beyond the rendered snapshot already
  tracking `modes`.

**Tests:** 2 LineMux cases, 1 grid case that feeds
`?2026h` + 10 frames + `?2026l` and asserts exactly one observable
state change.

**Estimate:** 1 commit, ~150 LOC + tests.

**Risk:** pending-buffer must never leak on drop / resize / error.
Watchdog timer covers the TUI-bug case.

### D2 — Dirty-flag rAF coalescing

Right now every animation frame takes a full `snapshot()` copy +
`setState` even if nothing changed between rAF cycles. On idle
panes this burns CPU for nothing; on a pane with a hidden animation
(status spinners, fsevents) it scales poorly across many panes.

**Cite:** alacritty's `Term::damage` tracking + the performance
notes from my post-session study.

**Implementation sketch:**
- `TerminalGrid` gets a `private dirty_: boolean = false`.
- Every public mutating method (`applyFrame`, `resize`) sets
  `dirty_ = true`.
- `snapshot()` stays the same, but `TerminalGrid` exposes
  `isDirty()` + `clearDirty()`.
- `SessionStreamView.scheduleRender`'s rAF callback becomes:
  `if (!gridRef.current.isDirty()) return;` + `clearDirty()` before
  `setSnapshot`.

**Tests:** 1 grid case that asserts dirty bit transitions, 1
SessionStreamView integration that a no-mutation rAF doesn't
rerender.

**Estimate:** 1 commit, ~40 LOC + tests.

**Risk:** miss a mutation site → stale rendering. Mitigation: only
two callers (`applyFrame`, `resize`); grep post-hoc to verify.

### D3 — Line-level damage tracking

Sibling of D2. Even when the grid *is* dirty, most rows don't
change each rAF. We currently re-run `renderRow` for every row
every render cycle. With memoization + damage bitmap, idle rows
stay identity-equal and React skips them.

**Cite:** alacritty's per-line damage in `Term::damaged_lines`.

**Implementation sketch:**
- `TerminalGrid`: track `Set<number>` of dirty row indices since
  last snapshot. Every row mutation adds to it.
- `snapshot()` returns `damagedRows: readonly number[]` alongside
  the grid.
- `renderRow` becomes memoized per `(row, damagedRows.has(i))`
  with rows that didn't change reusing prior React elements via a
  `useMemo` keyed on row identity.

**Tests:** 1 grid case that verifies the damage set across typical
mutation sequences.

**Estimate:** 1 commit, ~80 LOC + tests.

**Risk:** missed damage → stale visible row. Mitigate by pairing
with D2: if the grid is dirty globally but the damage set is empty,
damage must have been forgotten — log in dev mode.

### D4 — WS frame batching per rAF

Today `KesselClient.onFrame` dispatches each frame individually.
Claude's bottom-border repaints emit ~100 frames in one burst;
each triggers a separate `applyFrame` + `scheduleRender` call.
Batching inside the client — accumulate, flush on rAF — smooths
the hot path without changing semantics.

**Cite:** alacritty's damage-then-render inversion; same principle.

**Implementation sketch:**
- In `KesselClient`, buffer frames arriving between rAF ticks.
- Flush via a single rAF-scheduled callback that invokes
  `onFrame(batch)` — change the subscriber signature to `Frame[]`.
- `SessionStreamView` calls `applyFrame` in a loop then
  `scheduleRender` *once*.

**Tests:** 1 client case that feeds 10 frames inside one rAF and
asserts exactly one `onFrame` delivery.

**Estimate:** 1 commit, ~60 LOC + tests + subscriber migration.

**Risk:** changes the `KesselClient` API shape; any other consumer
(scripts/kessel-capture.ts) needs updating in the same commit.

### D5 — Application Cursor Keys (`DECSET ?1`)

Zsh, vim, and most readline-based shells flip to APP_CURSOR mode
on startup. The current Kessel pane passes `mode = 0` into
`keyEventToSequence`, which means arrow keys in zsh send the wrong
format. Users won't notice until they hit "up-arrow to get
previous command" and it types `^[[A` into the buffer.

**Cite:** existing `MODE_APP_CURSOR` flag in `key-mapping.ts` +
the comment in `SessionStreamView` acknowledging the gap.

**Implementation sketch:**
- Add `ModeKind::ApplicationCursor`.
- LineMux: dispatch `?1 h` / `?1 l` as `ModeChange`.
- `TerminalGrid.modes.appCursor: boolean`.
- `SessionStreamView`: read `gridRef.current?.snapshot().modes`
  inside the keydown handler and pass the OR'd flag bits to
  `keyEventToSequence`.

**Tests:** 2 LineMux cases, 1 grid case, 1 SessionStreamView
integration (stub `fetch`, verify CSI-vs-SS3 arrow key encoding
flips with mode).

**Estimate:** 1 commit, ~50 LOC + tests.

**Risk:** minimal. The flag is advisory; worst case it matches
today's behavior.

### D6 — Autowrap mode (`DECSET ?7`)

Some TUIs (rare ones, but Rosson will find them eventually) disable
wrap to draw at exact column positions. Our grid unconditionally
wraps at EOL, so those TUIs' right-column output smears into the
next line.

**Cite:** alacritty `TermMode::LINE_WRAP`. Low-impact today because
nothing we run daily disables it, but it's a correctness bug waiting
to be reported.

**Implementation sketch:**
- Add `ModeKind::Autowrap` (default true).
- LineMux: dispatch `?7 h`/`?7 l`.
- `TerminalGrid.writeChar`: when `modes_.autowrap === false`, clamp
  `cursor_.col` at `cols_ - 1` instead of wrapping.

**Tests:** 1 grid case that writes past EOL with wrap off and
asserts no row advance.

**Estimate:** 1 commit, ~30 LOC + tests.

**Risk:** minimal.

### D7 — Focus reporting (`DECSET ?1004`)

TUIs that honor focus-in/focus-out can dim the UI when the pane
isn't focused (neovim does, tmux does). Small polish; unblocks a
nice visual when a user tabs between panes.

**Cite:** alacritty `TermMode::FOCUS_IN_OUT`. Also mentioned in
`.k2so/notes/phase-4.5-kessel-complete.md` as a Phase 5 item.

**Implementation sketch:**
- Add `ModeKind::FocusReporting`.
- LineMux: dispatch `?1004 h`/`?1004 l`.
- `SessionStreamView`: attach `focus` / `blur` listeners to the
  container; when `modes.focusReporting`, write `\x1b[I` on focus
  and `\x1b[O` on blur.

**Tests:** 2 LineMux cases, 1 integration test that verifies the
focus/blur dispatch.

**Estimate:** 1 commit, ~40 LOC + tests.

**Risk:** minimal.

### D8 — Mouse reporting (`DECSET ?1000` / `?1006`, and ?1002/?1003)

⭐ **Biggest user-facing gap remaining.** Without this, clicking
inside htop/vim/less does nothing. Scroll inside vim doesn't go to
vim. Text selection in TUIs like lazygit is impossible.

**Cite:** alacritty `input::Mouse` handler + the roadmap item from
the post-session study.

**Implementation sketch (this is the one real project of 4.6):**
- Add `ModeKind` variants: `MouseX10`, `MouseVT200`,
  `MouseButtonEvent`, `MouseAnyEvent`, `MouseSGR`.
- LineMux: dispatch `?1000 h/l`, `?1002 h/l`, `?1003 h/l`,
  `?1006 h/l`.
- `TerminalGrid.modes`: `mouseMode` + `mouseProtocol` (SGR vs X10).
- `SessionStreamView`: wire `mousedown`/`mouseup`/`mousemove`/
  `wheel` → `mouseEventToSequence` encoder (new file
  `src/renderer/lib/mouse-mapping.ts`). Encoding depends on both
  mode *and* protocol; SGR is the modern default.
- When alt-screen is on, wheel events route to the TUI via mouse
  reporting instead of our scrollback viewport (the existing
  alt-screen-suppress-wheel guard becomes the dispatch point).

**Tests:** full mouse-encoding test suite (X10 vs SGR, button
combinations, wheel events), 1 integration that validates a click
dispatches through to `/cli/terminal/write`.

**Estimate:** 3-4 commits, ~400 LOC + tests. Biggest item in 4.6.

**Risk:** highest in the phase. Encoding bugs would confuse
TUIs subtly. Mitigation: mirror alacritty's encoder exactly + add
a golden-output test file with known-good byte sequences from
real TUIs (captured from the existing AlacrittyTerminalView path).

### D9 — OSC 0/1/2 — Window title

TUIs set the terminal title to show the current dir, agent, or
activity. Our tab label is stale; we currently drop all OSC
sequences. Having live titles gives users a visual signal of what
a tab is doing without switching to it.

**Cite:** alacritty `event::set_title` handler. Very cheap;
user-visible.

**Implementation sketch:**
- New frame variant `Frame::SetTitle(String)` OR overload
  `SemanticKind::Custom { kind: "title", … }`. Preference: the
  former — titles are common enough to earn a top-level variant.
- LineMux: in `osc_dispatch`, recognize params `0`, `1`, `2` (0/2
  both set the window title; 1 sets the icon title).
- Renderer: emit the event up to the tab owner via a new
  `SessionStreamViewProps.onTitle?: (title: string) => void`.
  Harness Lab ignores it; tab panes set the label.

**Tests:** 1 LineMux case, 1 integration case asserting the
callback fires with the correct decoded string.

**Estimate:** 1 commit, ~80 LOC + tests.

**Risk:** minimal. Title is advisory.

### D10 — OSC 4 / 10 / 11 — Palette override

Themes (solarized, dracula, etc.) reconfigure the 16-color palette
at startup via OSC 4. Our pane is locked to the compiled-in
Alacritty defaults. Lets users bring their terminal theme with
them.

**Cite:** alacritty `config::colors` dynamic update path.

**Implementation sketch:**
- New frame variant `Frame::PaletteUpdate { index: u8, rgb: u32 }`
  + variants for foreground/background (indices 10/11).
- LineMux `osc_dispatch`: parse `OSC 4 ; index ; rgb:RRRR/GGGG/BBBB`
  + simple `#RRGGBB` + named color parsing.
- `TerminalGrid.modes` (or sibling state): `palette_: Uint32Array(16)`.
- `styleToCss` in `style.ts`: when a cell's `fg`/`bg` is a palette
  index 0-15, resolve via current palette instead of the constant.
- Renderer: just passes through.

**Tests:** palette parser gets its own test file (3-4 cases covering
`rgb:` format, `#` format, named), 1 integration.

**Estimate:** 2 commits, ~200 LOC + tests.

**Risk:** parser fragility around edge formats. Mitigate by only
supporting the two common encodings + unit-testing them explicitly.

### D11 — OSC 8 — Hyperlinks

Modern CLIs (`gh`, some of Claude's outputs) emit clickable links
via OSC 8. We'd show them as clickable URLs instead of raw text.
Already latent in alacritty; real productivity win for CLI users.

**Cite:** alacritty `config::hyperlinks` + the roadmap item from
the study.

**Implementation sketch:**
- Extend `Style` with `hyperlink: Option<String>` (the URL).
- LineMux: track current-hyperlink state across `osc_dispatch`
  for OSC 8, apply to subsequent `Text` frames' styles.
- `renderRow` in `SessionStreamView`: spans with a non-null
  hyperlink become `<a>` tags with `href`.

**Tests:** 1 LineMux case that verifies hyperlink carries through
a `Text` frame's style.

**Estimate:** 1 commit, ~100 LOC + tests.

**Risk:** low. OSC 8 format is simple and well-specified.

### D12 — OSC 52 — Clipboard

`pbcopy`-from-a-TUI support. A TUI writes `OSC 52 ; c ; base64` and
we copy the decoded string to the system clipboard. Useful for
headless flows (the agent wants to put something in the user's
clipboard).

**Cite:** alacritty `config::selection.semantic_escape_chars`
supports this. Security-sensitive — guard by default.

**Implementation sketch:**
- New frame variant `Frame::ClipboardWrite(Vec<u8>)`.
- LineMux `osc_dispatch`: parse OSC 52, base64-decode.
- Renderer: forward to `navigator.clipboard.writeText` but gate
  behind a setting (`kessel.allowClipboardWrite`, default false).
  Security: agent could write anything, including commands that
  a naive user might paste into a shell. Default-off is mandatory.

**Tests:** 1 LineMux case, 1 integration verifying the setting gate.

**Estimate:** 1 commit, ~80 LOC + tests + one new setting.

**Risk:** medium — security implications. Requires a user-visible
setting toggle + rationale copy in the setting UI.

---

## Recommended sequencing

**Batch A — Polish & resilience (this week):**
`D1` → `D2` → `D3` → `D4`

Targets the cursor-jumping class of issue at its roots + the render
hot path. Self-contained; no new UX surfaces. Ship these before
anything user-visible, then re-baseline Rosson's "feels excellent"
vs. "feels world-class."

**Batch B — Input correctness (next week):**
`D5` → `D6` → `D7`

Flips the three small-but-correctness-bearing mode flags. `D5`
(APP_CURSOR) in particular fixes a bug the user probably hasn't
noticed yet — this is a pre-emptive fix.

**Batch C — Mouse (the real project of 4.6):**
`D8`

One-week focused effort. Can run in parallel with Batch B if the
team has capacity, but most productive solo because the encoder
layer is the integration bottleneck.

**Batch D — Niceties (last, land opportunistically):**
`D9` → `D10` → `D11` → `D12`

Each is independently-releasable. `D9` (window title) is probably
the most impactful of the four; `D12` (clipboard) is most
security-sensitive.

---

## Forward-compat with Phase 4.7 (word-editor tooling)

**Read `.k2so/notes/phase-4.7-word-editor-deferred.md` before
starting any 4.6 deliverable.** That doc defines seven binding
constraints (C1–C7) that 4.6 must honor so 4.7 stays buildable
without a rewrite. Highlights:

- **C2 per-cell click targeting** — don't introduce span
  coalescing tricks in `renderRow` that lose the ability to
  resolve col from click-x. Build `coordsFromMouseEvent` during
  D8; everything downstream inherits.
- **C3 archive is lossless** — D1 synchronized-output buffers on
  the renderer side only; the `SessionEntry` broadcast (upstream
  of both renderer and archive) sees every frame. D4 WS batching
  happens at the client boundary, not upstream of the entry.
- **C4 `SemanticEvent` frames preserved** — D2's dirty flag must
  set dirty on SemanticEvent receipt. D4 WS batches must preserve
  frame ordering.
- **C7 region-aware input dispatch** — D5 (keyboard) and D8
  (mouse) must expose optional region-dispatcher hooks. Default
  behavior is today's all-to-PTY; F6 rich prompt editor will
  swap in a dispatcher without touching D5/D8.

Every 4.6 commit message should cite which 4.7 constraints the
deliverable touches (format: `Honors 4.7 C2, C4`).

---

## Out of scope for 4.6 (and why)

- **OffscreenCanvas / GPU renderer.** Phase 8 territory. Can't be
  justified until the DOM renderer has measured performance issues
  that only a canvas approach would solve.
- **Full DECPRT / DECRQM mode-query support.** No TUI we run
  queries our mode state. Add when one does.
- **CSI window manipulation (`CSI t`).** TUIs rarely use it; resize
  reporting is handled via our own ResizeObserver.
- **Sixel / iTerm2 image protocols.** Nice-to-have but zero current
  asks and high implementation cost.
- **Cross-pane focus follow (multiple Kessel panes in one window).**
  Belongs to the tab-system refactor, not the renderer.

---

## Test baseline + exit criteria

**Entry:** 36 LineMux + 29 grid tests at the opening of 4.6.

**Exit (targets):**
- 60+ LineMux tests (every new mode gets 2 cases: set + reset).
- 45+ grid tests (every new mode flag + state effect).
- 1 mouse-encoding golden file with 10+ byte-level fixtures.
- 1 hyperlink integration test.
- 1 synchronized-output integration test asserting atomicity.
- Flag-off bit-for-bit identity check still passes.

**Definition of done per deliverable:**
- Rust-side tests green (`cargo test --features session_stream -p k2so-core`).
- TS typecheck clean (`bunx tsc --noEmit`).
- TS tests green (`bun test src/renderer/kessel/`).
- Manual smoke in the Harness Lab — the specific behavior the
  deliverable targets is observable.

---

## Known risks + mitigations

1. **Mode pile-up on `TerminalGrid.modes`.** We're accumulating
   flags fast. If it exceeds ~8 flags, reconsider as a bitmap
   instead of a struct — cheaper to pass across snapshot boundaries.
2. **`Frame::ModeChange` serialization grows.** Every variant must
   be mirrored in TS. A `ModeKind`-to-string mapping layer in the
   generator would be cheaper than hand-sync, but not worth it at
   4.6 scale.
3. **Mouse reporting bugs will feel catastrophic to the TUI.** Most
   severe risk in the phase. Ship D8 behind a config toggle during
   rollout (`kessel.mouseReporting: 'off' | 'basic' | 'full'`,
   default `off` for one release).
4. **Rosson keeps finding more things.** Good — his feedback is
   how this phase exists at all. Treat the plan as a living doc;
   add deliverables as D13, D14, ... rather than patching existing
   entries. Preserve the "what drove this" cite for each.

---

## Open questions for Rosson

1. Do you want D8 (mouse reporting) behind a setting for its first
   release, or just enabled by default? I'd default to on — TUIs
   opt into mouse mode themselves; we just respond — but a setting
   is cheap insurance.
2. Is OSC 52 (D12) something you want shipped behind off-by-default
   clipboard write permission, or do you want to default-allow
   (matches iTerm2's default)?
3. For D9 window titles, do you want the tab label to show the
   title, or do you want the title surfaced as a tooltip / secondary
   UI? My default: show in the tab label for now, add a setting
   later if it turns out noisy.
4. Any items from the 4.5 completion note you want to pull *back*
   into 4.6 rather than deferring to 5/6? The current 4.5
   completion list has a few "future work" items that overlap with
   this phase.

---

## Appendix — what "alacritty parity" means

When this phase is done, a user running the Kessel pane against
the same TUIs they run in Alacritty should see:
- Identical SGR coloring + cursor behavior.
- Identical response to mode sets — bracketed paste, alt screen,
  cursor visibility, app cursor, autowrap, focus reporting, mouse
  reporting, synchronized output.
- Identical OSC-driven theming + titles + hyperlinks (we don't
  need to match alacritty's image protocols, but the `osc_dispatch`
  floor should be congruent).

It does NOT mean:
- Identical rendering performance (DOM vs GPU is an inherent gap).
- Identical font rendering (browser cursive vs. FreeType differences).
- Identical selection semantics (we get browser-native selection;
  alacritty has its own).

The north star is: *if Rosson switches from Alacritty to Kessel
for a full workday and doesn't notice anything missing, we're done
with 4.6.*

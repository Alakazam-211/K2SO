// Kessel — TerminalGrid state machine.
//
// Pure state: accepts `Frame` events, maintains a 2D cell buffer +
// cursor + scrollback + scroll region. No React, no DOM — the
// renderer layer (SessionStreamView.tsx, Phase 4.5 I5) projects
// this state to spans on each animation frame.
//
// Scope: handles what Phase 1 LineMux actually emits today —
// Frame::Text (with Style=null) + the Phase 1 CursorOp subset
// (Goto, Up/Down/Forward/Back, EraseInLine/Display, ClearScreen).
// Alt-screen / mode switches / DECSC / DECRC are stubs; they get
// wired when core's LineMux earns the corresponding emissions.
//
// Invariants:
//   - Cursor coordinates are 0-indexed internally. Frame::CursorOp
//     Goto is 1-indexed per ECMA-48; `handleCursorOp` converts.
//   - Rows/cols are always >= 1. Resize clamps at 1×1 minimum.
//   - Scrollback grows lossily: when it exceeds SCROLLBACK_CAP, the
//     oldest row is dropped. Default cap is 10_000 lines.
//   - UTF-8 decoding buffers partial multi-byte sequences across
//     Frame::Text boundaries via a stateful TextDecoder.

import type { CursorShape, Frame, Style } from './types'

export interface Cell {
  /** Grapheme this cell holds. Empty string on a blank cell. */
  char: string
  /** SGR styling. `null` = terminal default. */
  style: Style | null
}

export interface Cursor {
  row: number
  col: number
  visible: boolean
  /** Shape requested by the TUI via DECSCUSR (CSI Ps SP q). `null`
   *  when no TUI has issued the sequence — renderer falls back to
   *  `config.cursor.defaultShape`. */
  shape: CursorShape | null
}

export interface GridSnapshot {
  rows: number
  cols: number
  /** Live grid rows, top-to-bottom. Read-only view (copy on write). */
  grid: readonly (readonly Cell[])[]
  /** Scrollback, oldest first. */
  scrollback: readonly (readonly Cell[])[]
  cursor: Readonly<Cursor>
  /** 0-indexed inclusive scroll region: [top, bottom]. */
  scrollRegion: Readonly<{ top: number; bottom: number }>
  /** Terminal mode flags — populated by ModeChange frames. Consumers
   *  read these to adapt behavior (e.g. wrap paste in ESC[200~..201~
   *  when bracketedPaste is true). */
  modes: Readonly<ModeFlags>
  /** Live-grid row indices that were touched since the last
   *  `clearDirty()`. Renderers use this to skip React reconciliation
   *  for rows that haven't changed (D3 damage tracking). The array
   *  is a fresh copy per snapshot — safe to hold across renders.
   *  Empty array = whole grid can be reused. When viewportOffset is
   *  non-zero (scrolled into scrollback), callers should treat every
   *  row as potentially-damaged since the viewport-to-row mapping
   *  shifted. */
  damagedRows: readonly number[]
  /** Monotonic counter incremented on every Frame::Bell received.
   *  Renderers watch this for changes and trigger the visual flash /
   *  audio cue. A counter (rather than a boolean) lets multiple
   *  bells fire flashes in quick succession without a manual reset
   *  — React's useEffect on this value fires once per increment. */
  bellCount: number
}

export interface ModeFlags {
  /** DECSET ?2004 — when true, pasted text must be wrapped in
   *  ESC[200~ … ESC[201~ so the TUI distinguishes paste from typing. */
  bracketedPaste: boolean
  /** DECSET ?1049 / ?47 — true while the grid is showing the alt-screen
   *  buffer. Renderers suppress scrollback navigation while on alt
   *  screen (the TUI owns the whole viewport and scrolling through its
   *  history isn't meaningful — there isn't any). */
  altScreen: boolean
  /** DECSET ?2026 — synchronized output. When true, `applyFrame` is
   *  buffering frames internally; the next visible snapshot will be
   *  the atomic result of every frame received between `?2026 h` and
   *  the matching `?2026 l`. Surfaced in the snapshot so callers can
   *  short-circuit repaints while the buffer is open. */
  synchronizedOutput: boolean
  /** DECSET ?1 — application cursor keys. When true, SessionStreamView
   *  encodes arrow keys as SS3 sequences (`ESC O A`) instead of CSI
   *  (`ESC [ A`). zsh/vim depend on this for up-arrow-for-history
   *  and normal-mode navigation. */
  appCursor: boolean
  /** DECSET ?7 — autowrap. When true (default), writes past the right
   *  edge wrap to the next row. When false, the cursor clamps at the
   *  last column and subsequent writes overwrite it. */
  autowrap: boolean
  /** DECSET ?1004 — focus reporting. When true, SessionStreamView
   *  writes `ESC [ I` on focus-in and `ESC [ O` on focus-out so the
   *  TUI can react to pane focus changes. */
  focusReporting: boolean
}

export interface TerminalGridOpts {
  rows?: number
  cols?: number
  /** Max scrollback lines before the oldest is discarded. */
  scrollbackCap?: number
  /** Watchdog for DECSET ?2026 synchronized output. If the TUI
   *  opens a sync update and never closes it, we force-flush the
   *  pending buffer after this many ms so the pane can't wedge.
   *  150ms matches alacritty. Set to 0 to disable (the buffer
   *  will only ever drain on an explicit close). */
  syncUpdateTimeoutMs?: number
}

const DEFAULT_ROWS = 24
const DEFAULT_COLS = 80
const DEFAULT_SCROLLBACK_CAP = 10_000
const DEFAULT_SYNC_UPDATE_TIMEOUT_MS = 150

/** Shared empty-damage sentinel. Using `===` against this lets the
 *  renderer short-circuit "the whole pane is clean" as one pointer
 *  compare instead of iterating an empty array. */
const EMPTY_DAMAGE: readonly number[] = Object.freeze([])

function blankCell(): Cell {
  return { char: '', style: null }
}
function blankRow(cols: number): Cell[] {
  return Array.from({ length: cols }, blankCell)
}

export class TerminalGrid {
  private rows_: number
  private cols_: number
  private grid_: Cell[][]
  private scrollback_: Cell[][] = []
  private cursor_: Cursor = { row: 0, col: 0, visible: true, shape: null }
  private scrollRegion_: { top: number; bottom: number }
  private savedCursor_: { row: number; col: number } | null = null
  private altGrid_: Cell[][] | null = null
  private modes_: ModeFlags = {
    bracketedPaste: false,
    altScreen: false,
    synchronizedOutput: false,
    appCursor: false,
    // Autowrap defaults ON per ECMA-48 / xterm — TUIs that want to
    // draw at exact coordinates explicitly turn it off.
    autowrap: true,
    focusReporting: false,
  }
  private readonly scrollbackCap_: number
  private readonly syncUpdateTimeoutMs_: number
  /** Frames that arrived while modes_.synchronizedOutput was true.
   *  Drained atomically on the close directive, on a subsequent
   *  frame arriving past the timeout (auto-recover from a buggy
   *  TUI), or on explicit forceDrain(). */
  private syncPending_: Frame[] = []
  /** Unix-ms timestamp the current sync window opened at. null when
   *  not in sync mode. */
  private syncOpenedAt_: number | null = null
  /** Dirty bit for rAF coalescing. Set by any frame that reaches
   *  the mutation path; cleared by `clearDirty()` after the renderer
   *  reads `snapshot()`. Lets SessionStreamView skip re-reading the
   *  snapshot when nothing changed between rAF ticks. */
  private dirty_: boolean = false
  /** Per-row damage set. Live-grid row indices that mutated since
   *  the last `clearDirty()`. Paired with `dirty_`: `dirty_ = true`
   *  with `damagedRows_.size === 0` is valid (e.g. a SemanticEvent
   *  arrived — the rerender path fires but no row needs repainting).
   *  Renderers memoize non-damaged rows. D3. */
  private damagedRows_: Set<number> = new Set()
  /** Running count of Bell frames received. Exposed on snapshot so
   *  the renderer can trigger one flash per increment via useEffect. */
  private bellCount_: number = 0
  // Partial UTF-8 handling: Frame::Text bytes are UTF-8 but multi-
  // byte sequences may span frames. TextDecoder's `stream: true`
  // mode buffers trailing partials across decode() calls.
  private readonly decoder = new TextDecoder('utf-8', { fatal: false })

  constructor(opts: TerminalGridOpts = {}) {
    this.rows_ = Math.max(1, opts.rows ?? DEFAULT_ROWS)
    this.cols_ = Math.max(1, opts.cols ?? DEFAULT_COLS)
    this.scrollbackCap_ = Math.max(0, opts.scrollbackCap ?? DEFAULT_SCROLLBACK_CAP)
    this.syncUpdateTimeoutMs_ = Math.max(
      0,
      opts.syncUpdateTimeoutMs ?? DEFAULT_SYNC_UPDATE_TIMEOUT_MS,
    )
    this.grid_ = Array.from({ length: this.rows_ }, () => blankRow(this.cols_))
    this.scrollRegion_ = { top: 0, bottom: this.rows_ - 1 }
  }

  get rows(): number {
    return this.rows_
  }
  get cols(): number {
    return this.cols_
  }

  // ── Public entry point ──────────────────────────────────────────

  /** Apply a Frame. Returns nothing — callers read state via
   *  `snapshot()` on the next animation frame.
   *
   *  While `modes_.synchronizedOutput` is true, non-control frames
   *  are buffered and will apply atomically when the TUI emits the
   *  close directive (or when the watchdog timeout fires). The
   *  archive and broadcast channels (Rust-side) are untouched by
   *  this buffering — they always see the original frame stream
   *  (4.7 C3 lossless invariant).
   */
  applyFrame(frame: Frame): void {
    // Sync-output close is the hot path — drain first, then apply
    // the off transition. This keeps the buffer tight and avoids
    // a re-entry with a stale syncOpenedAt_.
    if (
      frame.frame === 'ModeChange' &&
      frame.data.mode === 'synchronized_output'
    ) {
      if (frame.data.on) {
        if (!this.modes_.synchronizedOutput) {
          this.modes_.synchronizedOutput = true
          this.syncOpenedAt_ = Date.now()
          // Mark dirty so the snapshot reflects the new mode flag
          // on the next rAF. SessionStreamView's watchdog effect
          // depends on reading this transition via snapshot state.
          this.dirty_ = true
        }
        return
      }
      // Close path: drain always marks dirty if there were buffered
      // frames; additionally mark dirty for the mode-flag transition
      // itself so an empty sync window still produces one rerender
      // that flips the flag back to false.
      this.dirty_ = true
      this.drainSyncPending()
      return
    }

    // If we're in sync mode, check the watchdog first. A buggy TUI
    // that opened sync and never closed would wedge the pane
    // otherwise.
    if (
      this.modes_.synchronizedOutput &&
      this.syncOpenedAt_ !== null &&
      this.syncUpdateTimeoutMs_ > 0 &&
      Date.now() - this.syncOpenedAt_ > this.syncUpdateTimeoutMs_
    ) {
      this.drainSyncPending()
      // Fall through so the current frame applies immediately on
      // the now-drained state.
    }

    if (this.modes_.synchronizedOutput) {
      this.syncPending_.push(frame)
      return
    }

    this.applyFrameImmediate(frame)
  }

  /** Force-flush any buffered sync frames and return to live mode.
   *  No-op when sync isn't active. Useful for callers that own a
   *  watchdog clock outside the grid (SessionStreamView runs a
   *  setTimeout to cover the silent-TUI case where no new frame
   *  arrives to trigger the internal watchdog). */
  forceDrain(): void {
    if (this.modes_.synchronizedOutput) {
      this.drainSyncPending()
    }
  }

  /** How many frames are currently buffered waiting for the sync
   *  close. Zero when not in sync mode. Exposed for tests + the
   *  watchdog. */
  pendingSyncCount(): number {
    return this.syncPending_.length
  }

  /** True if any mutation has occurred since the last `clearDirty()`.
   *  Used by SessionStreamView's rAF loop to skip re-snapshotting
   *  when nothing has changed — eliminates one setState + one
   *  snapshot allocation per idle frame. */
  isDirty(): boolean {
    return this.dirty_
  }

  /** Clear the dirty bit AND the per-row damage set. Callers should
   *  read `snapshot()` BEFORE clearing — otherwise a mutation
   *  landing between the two calls would be seen as "clean" on the
   *  next rAF, and the row's damage info would be lost. */
  clearDirty(): void {
    this.dirty_ = false
    this.damagedRows_.clear()
  }

  private markAllRowsDamaged(): void {
    for (let i = 0; i < this.rows_; i++) this.damagedRows_.add(i)
  }

  private drainSyncPending(): void {
    const pending = this.syncPending_
    this.syncPending_ = []
    this.modes_.synchronizedOutput = false
    this.syncOpenedAt_ = null
    for (const f of pending) {
      this.applyFrameImmediate(f)
    }
  }

  private applyFrameImmediate(frame: Frame): void {
    // Conservative dirty mark — every frame that reaches the apply
    // path flips the bit, including SemanticEvent + AgentSignal
    // which don't mutate grid state but which 4.7 subscribers will
    // eventually consume (4.7 C4: don't drop these). The rerender
    // when the grid-visible state hasn't changed is cheap — React
    // skips DOM updates when the snapshot identity hasn't changed.
    this.dirty_ = true
    switch (frame.frame) {
      case 'Text':
        this.writeText(frame.data.bytes, frame.data.style)
        break
      case 'CursorOp':
        this.handleCursorOp(frame.data)
        break
      case 'SemanticEvent':
      case 'AgentSignal':
        // These frames don't affect the grid — the renderer pane
        // routes them to toasts / side channels. TerminalGrid is
        // pure text-grid state.
        break
      case 'RawPtyFrame':
        // Opaque passthrough — consumers that want pixel-perfect
        // replay feed this into an emulator. The Kessel DOM
        // renderer doesn't.
        break
      case 'ModeChange':
        this.handleModeChange(frame.data.mode, frame.data.on)
        break
      case 'Bell':
        // Counter-based — the renderer watches the delta and fires
        // one flash per increment (useEffect on bellCount).
        this.bellCount_ += 1
        break
    }
  }

  private handleModeChange(mode: string, on: boolean): void {
    switch (mode) {
      case 'bracketed_paste':
        this.modes_.bracketedPaste = on
        break
      case 'alt_screen':
        // ?1049 enter also saves the cursor and clears alt; ?1049
        // exit restores the cursor and the main buffer. ?47 is the
        // older op without the cursor save/restore. Both surface
        // here as the same ModeKind — the callers in LineMux have
        // already normalized.
        if (on) {
          this.enterAltScreen()
        } else {
          this.exitAltScreen()
        }
        this.modes_.altScreen = on
        break
      case 'application_cursor':
        this.modes_.appCursor = on
        break
      case 'autowrap':
        this.modes_.autowrap = on
        break
      case 'focus_reporting':
        this.modes_.focusReporting = on
        break
      // Future modes land here as they earn LineMux support.
    }
  }

  /** Resize to the given dimensions. Preserves scrollback + grid
   *  rows up to the new row count (truncating bottom rows or
   *  padding with blanks). Cursor clamps to new bounds. */
  resize(cols: number, rows: number): void {
    const newCols = Math.max(1, cols)
    const newRows = Math.max(1, rows)
    this.dirty_ = true
    // Any geometry change invalidates every visible row — row
    // indices shift, widths change, etc. Memoization must not
    // short-circuit here.
    // Mark before mutating rows_ so the loop uses the larger of
    // old/new for safety.
    for (let i = 0; i < Math.max(this.rows_, newRows); i++) {
      this.damagedRows_.add(i)
    }
    // Grow/shrink each existing row's column count.
    this.grid_ = this.grid_.map((row) => this.resizeRow(row, newCols))
    // Adjust row count: append blanks or trim from the bottom.
    if (newRows > this.rows_) {
      for (let i = this.rows_; i < newRows; i++) {
        this.grid_.push(blankRow(newCols))
      }
    } else if (newRows < this.rows_) {
      // Bottom rows may hold content — push them to scrollback so
      // the user can scroll up to see them after shrinking.
      const overflow = this.grid_.splice(newRows)
      for (const row of overflow) this.pushScrollback(row)
    }
    this.cols_ = newCols
    this.rows_ = newRows
    this.scrollRegion_ = { top: 0, bottom: this.rows_ - 1 }
    this.cursor_.row = Math.min(this.cursor_.row, this.rows_ - 1)
    this.cursor_.col = Math.min(this.cursor_.col, this.cols_ - 1)
  }

  snapshot(): GridSnapshot {
    return {
      rows: this.rows_,
      cols: this.cols_,
      grid: this.grid_,
      scrollback: this.scrollback_,
      cursor: { ...this.cursor_ },
      scrollRegion: { ...this.scrollRegion_ },
      modes: { ...this.modes_ },
      damagedRows:
        this.damagedRows_.size === 0
          ? EMPTY_DAMAGE
          : Array.from(this.damagedRows_),
      bellCount: this.bellCount_,
    }
  }

  // ── Mode stubs (wire when LineMux emits these) ──────────────────

  /** Save cursor position (DECSC). Exposed for the future mode-
   *  switch hook even though Phase 1 LineMux doesn't emit it. */
  saveCursor(): void {
    this.savedCursor_ = { row: this.cursor_.row, col: this.cursor_.col }
  }
  /** Restore cursor position (DECRC). No-op if no save exists. */
  restoreCursor(): void {
    if (!this.savedCursor_) return
    this.cursor_.row = Math.min(this.savedCursor_.row, this.rows_ - 1)
    this.cursor_.col = Math.min(this.savedCursor_.col, this.cols_ - 1)
  }

  /** Enter alternate-screen buffer (CSI ?1049h). Phase 1 LineMux
   *  doesn't emit this yet; vim / less / htop need it for parity.
   *  Wired here so the renderer layer can swap to alt buffer the
   *  moment LineMux learns the mode. */
  enterAltScreen(): void {
    if (this.altGrid_) return
    this.altGrid_ = this.grid_
    this.grid_ = Array.from({ length: this.rows_ }, () => blankRow(this.cols_))
    this.cursor_.row = 0
    this.cursor_.col = 0
    this.markAllRowsDamaged()
  }
  /** Exit alt screen, restore the primary buffer unchanged. */
  exitAltScreen(): void {
    if (!this.altGrid_) return
    this.grid_ = this.altGrid_
    this.altGrid_ = null
    this.markAllRowsDamaged()
  }
  /** Whether we're currently on the alt-screen buffer. */
  onAltScreen(): boolean {
    return this.altGrid_ !== null
  }

  // ── Text write path ─────────────────────────────────────────────

  private writeText(bytes: number[], style: Style | null): void {
    const text = this.decoder.decode(Uint8Array.from(bytes), { stream: true })
    // Iterate by code point (string iterator). Intl.Segmenter in
    // "grapheme" mode treats CRLF as a single cluster, which
    // breaks control-char dispatch — \r and \n need separate
    // writeChar calls. Combining marks end up as separate cells
    // for now; Phase 5+ can add Unicode width awareness if a
    // harness requires it.
    for (const char of text) this.writeChar(char, style)
  }

  private writeChar(char: string, style: Style | null): void {
    // Control chars first.
    if (char === '\n') {
      this.lineFeed()
      return
    }
    if (char === '\r') {
      this.cursor_.col = 0
      return
    }
    if (char === '\b') {
      if (this.cursor_.col > 0) this.cursor_.col -= 1
      return
    }
    if (char === '\t') {
      // 8-column tab stops.
      const next = (Math.floor(this.cursor_.col / 8) + 1) * 8
      this.cursor_.col = Math.min(next, this.cols_ - 1)
      return
    }
    // Wrap + scroll at EOL — unless autowrap is off (DECRST ?7),
    // in which case the cursor clamps at the last column and
    // subsequent writes overwrite that cell in place.
    if (this.cursor_.col >= this.cols_) {
      if (this.modes_.autowrap) {
        this.cursor_.col = 0
        this.lineFeed()
      } else {
        this.cursor_.col = this.cols_ - 1
      }
    }
    this.grid_[this.cursor_.row][this.cursor_.col] = { char, style }
    this.damagedRows_.add(this.cursor_.row)
    // In wrap-off mode we do NOT advance past the last column; the
    // next write will land on the same cell. In wrap-on mode we
    // advance normally — the next write triggers the wrap/scroll
    // branch above.
    if (this.modes_.autowrap || this.cursor_.col < this.cols_ - 1) {
      this.cursor_.col += 1
    }
  }

  private lineFeed(): void {
    if (this.cursor_.row < this.scrollRegion_.bottom) {
      this.cursor_.row += 1
      return
    }
    // At bottom of scroll region — scroll up one line. The departed
    // top row goes to scrollback when the region covers the full
    // screen; inside a partial region, it's discarded (ECMA-48 §8.3.80).
    const covers_full = this.scrollRegion_.top === 0 &&
      this.scrollRegion_.bottom === this.rows_ - 1
    const top = this.scrollRegion_.top
    const bottom = this.scrollRegion_.bottom
    const departing = this.grid_[top]
    if (covers_full) this.pushScrollback(departing)
    for (let r = top; r < bottom; r++) {
      this.grid_[r] = this.grid_[r + 1]
      this.damagedRows_.add(r)
    }
    this.grid_[bottom] = blankRow(this.cols_)
    this.damagedRows_.add(bottom)
    // Cursor stays at same absolute row (bottom of region).
  }

  private pushScrollback(row: Cell[]): void {
    if (this.scrollbackCap_ === 0) return
    this.scrollback_.push(row)
    if (this.scrollback_.length > this.scrollbackCap_) {
      this.scrollback_.shift()
    }
  }

  // ── CursorOp dispatch ───────────────────────────────────────────

  private handleCursorOp(op: Frame & { frame: 'CursorOp' } extends {
    frame: 'CursorOp'
    data: infer D
  }
    ? D
    : never): void {
    switch (op.op) {
      case 'Goto':
        // ECMA-48 CUP is 1-indexed; convert to 0-indexed.
        this.cursor_.row = this.clampRow(op.value.row - 1)
        this.cursor_.col = this.clampCol(op.value.col - 1)
        break
      case 'Up':
        this.cursor_.row = this.clampRow(this.cursor_.row - op.value)
        break
      case 'Down':
        this.cursor_.row = this.clampRow(this.cursor_.row + op.value)
        break
      case 'Forward':
        this.cursor_.col = this.clampCol(this.cursor_.col + op.value)
        break
      case 'Back':
        this.cursor_.col = this.clampCol(this.cursor_.col - op.value)
        break
      case 'EraseInLine':
        this.eraseInLine(op.value)
        break
      case 'EraseInDisplay':
        this.eraseInDisplay(op.value)
        break
      case 'ClearScreen':
        this.clearScreen()
        break
      case 'SaveCursor':
        this.saveCursor()
        break
      case 'RestoreCursor':
        this.restoreCursor()
        break
      case 'SetCursorVisible':
        this.cursor_.visible = op.value
        break
      case 'SetCursorStyle':
        this.cursor_.shape = op.value
        break
    }
  }

  private clampRow(r: number): number {
    if (r < 0) return 0
    if (r > this.rows_ - 1) return this.rows_ - 1
    return r
  }
  private clampCol(c: number): number {
    if (c < 0) return 0
    if (c > this.cols_ - 1) return this.cols_ - 1
    return c
  }

  private eraseInLine(mode: 'to_end' | 'from_start' | 'all'): void {
    const row = this.grid_[this.cursor_.row]
    const col = this.cursor_.col
    switch (mode) {
      case 'to_end':
        for (let c = col; c < this.cols_; c++) row[c] = blankCell()
        break
      case 'from_start':
        for (let c = 0; c <= col; c++) row[c] = blankCell()
        break
      case 'all':
        for (let c = 0; c < this.cols_; c++) row[c] = blankCell()
        break
    }
    this.damagedRows_.add(this.cursor_.row)
  }

  private eraseInDisplay(mode: 'to_end' | 'from_start' | 'all'): void {
    switch (mode) {
      case 'to_end':
        this.eraseInLine('to_end')
        for (let r = this.cursor_.row + 1; r < this.rows_; r++) {
          this.grid_[r] = blankRow(this.cols_)
          this.damagedRows_.add(r)
        }
        break
      case 'from_start':
        for (let r = 0; r < this.cursor_.row; r++) {
          this.grid_[r] = blankRow(this.cols_)
          this.damagedRows_.add(r)
        }
        this.eraseInLine('from_start')
        break
      case 'all':
        this.clearScreen()
        break
    }
  }

  private clearScreen(): void {
    this.grid_ = Array.from({ length: this.rows_ }, () => blankRow(this.cols_))
    this.cursor_.row = 0
    this.cursor_.col = 0
    this.markAllRowsDamaged()
  }

  private resizeRow(row: Cell[], newCols: number): Cell[] {
    if (row.length === newCols) return row
    if (row.length > newCols) return row.slice(0, newCols)
    return row.concat(
      Array.from({ length: newCols - row.length }, blankCell),
    )
  }
}

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

import type { Frame, Style } from './types'

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
}

export interface TerminalGridOpts {
  rows?: number
  cols?: number
  /** Max scrollback lines before the oldest is discarded. */
  scrollbackCap?: number
}

const DEFAULT_ROWS = 24
const DEFAULT_COLS = 80
const DEFAULT_SCROLLBACK_CAP = 10_000

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
  private cursor_: Cursor = { row: 0, col: 0, visible: true }
  private scrollRegion_: { top: number; bottom: number }
  private savedCursor_: { row: number; col: number } | null = null
  private altGrid_: Cell[][] | null = null
  private readonly scrollbackCap_: number
  // Partial UTF-8 handling: Frame::Text bytes are UTF-8 but multi-
  // byte sequences may span frames. TextDecoder's `stream: true`
  // mode buffers trailing partials across decode() calls.
  private readonly decoder = new TextDecoder('utf-8', { fatal: false })

  constructor(opts: TerminalGridOpts = {}) {
    this.rows_ = Math.max(1, opts.rows ?? DEFAULT_ROWS)
    this.cols_ = Math.max(1, opts.cols ?? DEFAULT_COLS)
    this.scrollbackCap_ = Math.max(0, opts.scrollbackCap ?? DEFAULT_SCROLLBACK_CAP)
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
   *  `snapshot()` on the next animation frame. */
  applyFrame(frame: Frame): void {
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
    }
  }

  /** Resize to the given dimensions. Preserves scrollback + grid
   *  rows up to the new row count (truncating bottom rows or
   *  padding with blanks). Cursor clamps to new bounds. */
  resize(cols: number, rows: number): void {
    const newCols = Math.max(1, cols)
    const newRows = Math.max(1, rows)
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
  }
  /** Exit alt screen, restore the primary buffer unchanged. */
  exitAltScreen(): void {
    if (!this.altGrid_) return
    this.grid_ = this.altGrid_
    this.altGrid_ = null
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
    // Wrap + scroll at EOL.
    if (this.cursor_.col >= this.cols_) {
      this.cursor_.col = 0
      this.lineFeed()
    }
    this.grid_[this.cursor_.row][this.cursor_.col] = { char, style }
    this.cursor_.col += 1
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
    }
    this.grid_[bottom] = blankRow(this.cols_)
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
  }

  private eraseInDisplay(mode: 'to_end' | 'from_start' | 'all'): void {
    switch (mode) {
      case 'to_end':
        this.eraseInLine('to_end')
        for (let r = this.cursor_.row + 1; r < this.rows_; r++) {
          this.grid_[r] = blankRow(this.cols_)
        }
        break
      case 'from_start':
        for (let r = 0; r < this.cursor_.row; r++) {
          this.grid_[r] = blankRow(this.cols_)
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
  }

  private resizeRow(row: Cell[], newCols: number): Cell[] {
    if (row.length === newCols) return row
    if (row.length > newCols) return row.slice(0, newCols)
    return row.concat(
      Array.from({ length: newCols - row.length }, blankCell),
    )
  }
}

// Kessel — selection geometry + serialization.
//
// Pure utilities (no React, no DOM access). The pane component
// tracks mouse positions and passes them here; this module
// normalizes the range, computes per-row highlight extents, and
// walks the grid to produce the clipboard payload.
//
// Selection model: linear (not rectangular) — dragging from
// (5, 20) to (10, 30) selects:
//   - row 5 from col 20 to end-of-row
//   - rows 6-9 fully
//   - row 10 from col 0 to col 30 (inclusive)
// Matches alacritty + Terminal.app + iTerm2 default.

import type { Cell } from './grid'

export interface GridPoint {
  row: number
  col: number
}

/** Inclusive selection range, pre-normalization. The pane reports
 *  anchor (mousedown) and head (current mouse) coordinates as-is. */
export interface RawSelection {
  anchor: GridPoint
  head: GridPoint
}

/** Per-row selection extent after normalization. `start`/`end` are
 *  inclusive column indices. */
export interface RowSelection {
  row: number
  start: number
  end: number
}

/** Normalized selection — anchor/head swapped so `start` is always
 *  before `end` in reading order (top-to-bottom, left-to-right). */
export interface NormalizedSelection {
  start: GridPoint
  end: GridPoint
}

/** Normalize a raw selection so start <= end in reading order. */
export function normalize(sel: RawSelection): NormalizedSelection {
  const { anchor, head } = sel
  if (anchor.row < head.row) {
    return { start: anchor, end: head }
  }
  if (anchor.row > head.row) {
    return { start: head, end: anchor }
  }
  // Same row — compare cols.
  if (anchor.col <= head.col) {
    return { start: anchor, end: head }
  }
  return { start: head, end: anchor }
}

/** Expand a normalized selection into per-row extents. Each row
 *  intersected by the selection gets one entry with inclusive
 *  column bounds. */
export function rowExtents(
  sel: NormalizedSelection,
  cols: number,
): RowSelection[] {
  const { start, end } = sel
  const out: RowSelection[] = []
  if (start.row === end.row) {
    out.push({ row: start.row, start: start.col, end: end.col })
    return out
  }
  // First row: from start.col to end-of-row.
  out.push({ row: start.row, start: start.col, end: cols - 1 })
  // Middle rows: fully selected.
  for (let r = start.row + 1; r < end.row; r++) {
    out.push({ row: r, start: 0, end: cols - 1 })
  }
  // Last row: from 0 to end.col.
  out.push({ row: end.row, start: 0, end: end.col })
  return out
}

/** Serialize a selection to plain text for the clipboard. Walks
 *  the grid row-by-row, concatenating cell chars, trimming trailing
 *  whitespace per row (alacritty convention so users don't paste
 *  a forest of spaces from a half-blank line). Rows joined with \n. */
export function serialize(
  grid: readonly (readonly Cell[])[],
  sel: NormalizedSelection,
  cols: number,
): string {
  const extents = rowExtents(sel, cols)
  const lines: string[] = []
  for (const ext of extents) {
    const row = grid[ext.row]
    if (!row) continue
    // Slice inclusively. Empty cells render as a space so a run of
    // blank cells mid-text round-trips correctly; trailing blanks
    // get trimmed at the end.
    const chars: string[] = []
    for (let c = ext.start; c <= ext.end && c < row.length; c++) {
      chars.push(row[c].char || ' ')
    }
    lines.push(chars.join('').replace(/ +$/, ''))
  }
  return lines.join('\n')
}

// ── Word / line selection helpers ───────────────────────────────

/** Consider this set of characters as "word" content. Symbols like
 *  `_`, `-`, `.`, `/` are word-joining so path fragments double-
 *  click as one unit (alacritty-style). Adjust the regex if a
 *  harness needs a different boundary set. */
const WORD_CHAR = /[a-zA-Z0-9_\-./:@~]/

/** Expand a single point to a word-boundary selection on its row.
 *  Returns the same point if the cell under the cursor isn't a
 *  word character. */
export function wordAt(
  grid: readonly (readonly Cell[])[],
  p: GridPoint,
): NormalizedSelection {
  const row = grid[p.row]
  if (!row || !row[p.col] || !WORD_CHAR.test(row[p.col].char)) {
    return { start: p, end: p }
  }
  let start = p.col
  while (start > 0 && WORD_CHAR.test(row[start - 1].char)) start -= 1
  let end = p.col
  while (end < row.length - 1 && WORD_CHAR.test(row[end + 1].char)) end += 1
  return {
    start: { row: p.row, col: start },
    end: { row: p.row, col: end },
  }
}

/** Expand a single point to the whole row (line-selection). */
export function lineAt(cols: number, p: GridPoint): NormalizedSelection {
  return {
    start: { row: p.row, col: 0 },
    end: { row: p.row, col: cols - 1 },
  }
}

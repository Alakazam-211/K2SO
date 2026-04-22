import { describe, it, expect } from 'vitest'
import {
  normalize,
  rowExtents,
  serialize,
  wordAt,
  lineAt,
  type RawSelection,
  type GridPoint,
} from './selection'
import type { Cell } from './grid'

function cell(ch: string): Cell {
  return { char: ch, style: null }
}
function row(text: string, cols: number): Cell[] {
  const arr: Cell[] = []
  for (let i = 0; i < cols; i++) arr.push(cell(text[i] ?? ''))
  return arr
}

describe('normalize', () => {
  it('passes through anchor-before-head on same row', () => {
    const sel: RawSelection = {
      anchor: { row: 2, col: 3 },
      head: { row: 2, col: 7 },
    }
    expect(normalize(sel)).toEqual({
      start: { row: 2, col: 3 },
      end: { row: 2, col: 7 },
    })
  })

  it('swaps when head is before anchor on same row', () => {
    const sel: RawSelection = {
      anchor: { row: 2, col: 9 },
      head: { row: 2, col: 2 },
    }
    expect(normalize(sel)).toEqual({
      start: { row: 2, col: 2 },
      end: { row: 2, col: 9 },
    })
  })

  it('swaps when head is on an earlier row', () => {
    const sel: RawSelection = {
      anchor: { row: 5, col: 3 },
      head: { row: 2, col: 7 },
    }
    expect(normalize(sel)).toEqual({
      start: { row: 2, col: 7 },
      end: { row: 5, col: 3 },
    })
  })
})

describe('rowExtents', () => {
  it('same-row selection produces one extent', () => {
    const out = rowExtents(
      { start: { row: 3, col: 2 }, end: { row: 3, col: 7 } },
      20,
    )
    expect(out).toEqual([{ row: 3, start: 2, end: 7 }])
  })

  it('multi-row selection spans first + middle + last rows', () => {
    const out = rowExtents(
      { start: { row: 1, col: 5 }, end: { row: 4, col: 2 } },
      10,
    )
    expect(out).toEqual([
      { row: 1, start: 5, end: 9 }, // first: start.col → end-of-row
      { row: 2, start: 0, end: 9 }, // full
      { row: 3, start: 0, end: 9 }, // full
      { row: 4, start: 0, end: 2 }, // last: 0 → end.col
    ])
  })

  it('two-row selection produces first + last (no middles)', () => {
    const out = rowExtents(
      { start: { row: 1, col: 5 }, end: { row: 2, col: 3 } },
      10,
    )
    expect(out).toEqual([
      { row: 1, start: 5, end: 9 },
      { row: 2, start: 0, end: 3 },
    ])
  })
})

describe('serialize', () => {
  it('copies a same-row slice', () => {
    const grid = [row('hello world', 12)]
    const out = serialize(
      grid,
      { start: { row: 0, col: 6 }, end: { row: 0, col: 10 } },
      12,
    )
    expect(out).toBe('world')
  })

  it('joins multi-row selection with \\n + trims trailing spaces', () => {
    const grid = [
      row('line one   ', 12), // trailing blanks
      row('line two', 12),
    ]
    const out = serialize(
      grid,
      { start: { row: 0, col: 0 }, end: { row: 1, col: 7 } },
      12,
    )
    expect(out).toBe('line one\nline two')
  })

  it('handles empty cells mid-selection as spaces', () => {
    const grid = [row('ab', 5)] // cells 2-4 are empty
    const out = serialize(
      grid,
      { start: { row: 0, col: 0 }, end: { row: 0, col: 4 } },
      5,
    )
    // Trailing spaces from empty cells get trimmed.
    expect(out).toBe('ab')
  })
})

describe('wordAt', () => {
  it('expands within alphanumeric run', () => {
    const grid = [row('say hello world', 20)]
    const sel = wordAt(grid, { row: 0, col: 6 })
    expect(sel).toEqual({
      start: { row: 0, col: 4 },
      end: { row: 0, col: 8 },
    })
  })

  it('includes path-joining chars (_-./:@~)', () => {
    const grid = [row('path/to/file.rs ok', 20)]
    const sel = wordAt(grid, { row: 0, col: 3 })
    // Expands from `/` outward since `/` is a word char here.
    expect(sel.start.col).toBe(0)
    expect(sel.end.col).toBe(14)
  })

  it('collapses to point when cell isn\'t a word char', () => {
    const grid = [row('a b c', 5)]
    // cell at col 1 is a space.
    const p: GridPoint = { row: 0, col: 1 }
    expect(wordAt(grid, p)).toEqual({ start: p, end: p })
  })
})

describe('lineAt', () => {
  it('expands to full row width', () => {
    expect(lineAt(80, { row: 5, col: 33 })).toEqual({
      start: { row: 5, col: 0 },
      end: { row: 5, col: 79 },
    })
  })
})

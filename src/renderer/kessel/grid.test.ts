// Unit tests for TerminalGrid state machine. One test per transition
// + a handful of end-to-end sequences that mimic what LineMux
// actually produces on real harnesses.
import { describe, it, expect } from 'vitest'
import { TerminalGrid } from './grid'
import type { Frame } from './types'

// ── Helpers ──────────────────────────────────────────────────────────

/** Text frame builder. Accepts a string; emits a Frame::Text carrying
 *  its UTF-8 bytes. Style is null for all Phase 1 emissions. */
function text(s: string): Frame {
  return {
    frame: 'Text',
    data: { bytes: Array.from(new TextEncoder().encode(s)), style: null },
  }
}

/** Read a snapshot row as a string. Trims trailing blanks for easy
 *  comparison; use `rawRow` if you need blank-aware fidelity. */
function rowText(g: TerminalGrid, row: number): string {
  const snap = g.snapshot()
  const cells = snap.grid[row]
  const s = cells.map((c) => c.char || ' ').join('')
  return s.replace(/ +$/, '')
}

function rawRow(g: TerminalGrid, row: number): string {
  const snap = g.snapshot()
  return snap.grid[row].map((c) => c.char || ' ').join('')
}

// ── Cursor + text write ──────────────────────────────────────────────

describe('TerminalGrid text write', () => {
  it('writes a simple string to row 0 and advances cursor', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame(text('hello'))
    expect(rowText(g, 0)).toBe('hello')
    expect(g.snapshot().cursor).toMatchObject({ row: 0, col: 5 })
  })

  it('handles \\r to reset column, \\n to advance row', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame(text('hi\r\nworld'))
    expect(rowText(g, 0)).toBe('hi')
    expect(rowText(g, 1)).toBe('world')
    expect(g.snapshot().cursor).toMatchObject({ row: 1, col: 5 })
  })

  it('wraps at end of line', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame(text('abcdefgh'))
    expect(rowText(g, 0)).toBe('abcde')
    expect(rowText(g, 1)).toBe('fgh')
    expect(g.snapshot().cursor).toMatchObject({ row: 1, col: 3 })
  })

  it('backspace moves cursor left without erasing', () => {
    const g = new TerminalGrid({ rows: 3, cols: 10 })
    g.applyFrame(text('abc\b'))
    expect(rowText(g, 0)).toBe('abc')
    expect(g.snapshot().cursor.col).toBe(2)
  })

  it('tab advances to next 8-column stop', () => {
    const g = new TerminalGrid({ rows: 3, cols: 20 })
    g.applyFrame(text('a\tb'))
    // 'a' at col 0 → cursor 1 → tab → col 8 → 'b' at col 8 → cursor 9.
    expect(g.snapshot().cursor.col).toBe(9)
    expect(rawRow(g, 0).slice(0, 10)).toBe('a       b ')
  })
})

// ── Scrollback on newline at bottom ─────────────────────────────────

describe('TerminalGrid scroll at bottom of screen', () => {
  it('pushes departing top row to scrollback when cursor wraps at last row', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame(text('aaaaa\r\nbbbbb\r\nccccc\r\nddddd'))
    const snap = g.snapshot()
    // Top row "aaaaa" scrolled into scrollback; live grid now
    // b/c/d.
    expect(snap.scrollback.length).toBe(1)
    expect(snap.scrollback[0].map((c) => c.char).join('')).toBe('aaaaa')
    expect(rowText(g, 0)).toBe('bbbbb')
    expect(rowText(g, 1)).toBe('ccccc')
    expect(rowText(g, 2)).toBe('ddddd')
  })

  it('drops oldest scrollback line when cap is reached', () => {
    const g = new TerminalGrid({ rows: 1, cols: 3, scrollbackCap: 2 })
    for (const s of ['AAA\r\n', 'BBB\r\n', 'CCC\r\n', 'DDD']) {
      g.applyFrame(text(s))
    }
    const snap = g.snapshot()
    expect(snap.scrollback.length).toBe(2)
    // Oldest dropped → scrollback holds [BBB, CCC] and live grid is DDD.
    const joined = snap.scrollback.map((r) =>
      r.map((c) => c.char).join(''),
    )
    expect(joined).toEqual(['BBB', 'CCC'])
    expect(rowText(g, 0)).toBe('DDD')
  })
})

// ── CursorOp variants ────────────────────────────────────────────────

describe('TerminalGrid CursorOp', () => {
  it('Goto converts 1-indexed to 0-indexed', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame({
      frame: 'CursorOp',
      data: { op: 'Goto', value: { row: 3, col: 5 } },
    })
    expect(g.snapshot().cursor).toMatchObject({ row: 2, col: 4 })
  })

  it('Goto clamps to grid bounds', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame({
      frame: 'CursorOp',
      data: { op: 'Goto', value: { row: 99, col: 99 } },
    })
    expect(g.snapshot().cursor).toMatchObject({ row: 2, col: 4 })
  })

  it('Up / Down / Forward / Back move cursor relative', () => {
    const g = new TerminalGrid({ rows: 10, cols: 10 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 5, col: 5 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Up', value: 2 } })
    expect(g.snapshot().cursor).toMatchObject({ row: 2, col: 4 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Down', value: 1 } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Forward', value: 3 } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Back', value: 1 } })
    expect(g.snapshot().cursor).toMatchObject({ row: 3, col: 6 })
  })

  it('EraseInLine to_end clears from cursor to end', () => {
    const g = new TerminalGrid({ rows: 3, cols: 10 })
    g.applyFrame(text('abcdefghij'))
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 1, col: 4 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'EraseInLine', value: 'to_end' } })
    expect(rawRow(g, 0)).toBe('abc       ')
  })

  it('EraseInLine from_start clears from start to cursor', () => {
    const g = new TerminalGrid({ rows: 3, cols: 10 })
    g.applyFrame(text('abcdefghij'))
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 1, col: 5 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'EraseInLine', value: 'from_start' } })
    // Cursor at col 4 (0-indexed) after Goto — col 1..5 → 0..4 inclusive.
    expect(rawRow(g, 0)).toBe('     fghij')
  })

  it('EraseInLine all clears the whole line', () => {
    const g = new TerminalGrid({ rows: 3, cols: 10 })
    g.applyFrame(text('abcdefghij'))
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 1, col: 5 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'EraseInLine', value: 'all' } })
    expect(rawRow(g, 0)).toBe('          ')
  })

  it('EraseInDisplay to_end clears current line tail + all rows below', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame(text('aaaaa\r\nbbbbb\r\nccccc'))
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 2, col: 3 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'EraseInDisplay', value: 'to_end' } })
    expect(rowText(g, 0)).toBe('aaaaa')
    expect(rawRow(g, 1)).toBe('bb   ')
    expect(rawRow(g, 2)).toBe('     ')
  })

  it('ClearScreen wipes grid + resets cursor to 0,0', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame(text('aaaaa\r\nbbbbb\r\nccccc'))
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 2, col: 3 } } })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'ClearScreen', value: null } })
    expect(rawRow(g, 0)).toBe('     ')
    expect(rawRow(g, 1)).toBe('     ')
    expect(rawRow(g, 2)).toBe('     ')
    expect(g.snapshot().cursor).toMatchObject({ row: 0, col: 0 })
  })
})

// ── Resize ───────────────────────────────────────────────────────────

describe('TerminalGrid resize', () => {
  it('grows cols by padding rows with blanks', () => {
    const g = new TerminalGrid({ rows: 2, cols: 5 })
    g.applyFrame(text('abc\r\ndef'))
    g.resize(10, 2)
    expect(rawRow(g, 0)).toBe('abc       ')
    expect(rawRow(g, 1)).toBe('def       ')
  })

  it('shrinks rows by pushing bottom rows into scrollback', () => {
    const g = new TerminalGrid({ rows: 3, cols: 5 })
    g.applyFrame(text('aaaaa\r\nbbbbb\r\nccccc'))
    g.resize(5, 2)
    const snap = g.snapshot()
    expect(snap.rows).toBe(2)
    expect(snap.scrollback.length).toBe(1)
    expect(snap.scrollback[0].map((c) => c.char).join('')).toBe('ccccc')
    expect(rowText(g, 0)).toBe('aaaaa')
    expect(rowText(g, 1)).toBe('bbbbb')
  })

  it('clamps cursor into new bounds', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 5, col: 10 } } })
    // Cursor ended up at (4,9) after clamping.
    g.resize(5, 3)
    expect(g.snapshot().cursor).toMatchObject({ row: 2, col: 4 })
  })
})

// ── Alt screen + save/restore cursor (hooks, no LineMux yet) ────────

describe('TerminalGrid mode stubs', () => {
  it('enter/exit alt screen swaps buffers and clears the alt buffer', () => {
    const g = new TerminalGrid({ rows: 2, cols: 5 })
    g.applyFrame(text('AAAAA\r\nBBBBB'))
    g.enterAltScreen()
    expect(g.onAltScreen()).toBe(true)
    expect(rawRow(g, 0)).toBe('     ')
    g.applyFrame(text('ZZZZZ'))
    expect(rawRow(g, 0)).toBe('ZZZZZ')
    g.exitAltScreen()
    expect(g.onAltScreen()).toBe(false)
    // Primary buffer restored unchanged.
    expect(rowText(g, 0)).toBe('AAAAA')
    expect(rowText(g, 1)).toBe('BBBBB')
  })

  it('saveCursor + restoreCursor round-trips', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 3, col: 5 } } })
    g.saveCursor()
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 1, col: 1 } } })
    g.restoreCursor()
    expect(g.snapshot().cursor).toMatchObject({ row: 2, col: 4 })
  })
})

// ── SaveCursor / RestoreCursor via CursorOp frames ─────────────────

describe('TerminalGrid SaveCursor / RestoreCursor CursorOps', () => {
  it('SaveCursor frame + RestoreCursor frame round-trips cursor position', () => {
    const g = new TerminalGrid({ rows: 10, cols: 20 })
    // Move cursor to (3, 5) then save.
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 4, col: 6 } } })
    expect(g.snapshot().cursor).toMatchObject({ row: 3, col: 5 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'SaveCursor', value: null } })
    // Move elsewhere + write a char.
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 8, col: 12 } } })
    expect(g.snapshot().cursor).toMatchObject({ row: 7, col: 11 })
    // Restore.
    g.applyFrame({ frame: 'CursorOp', data: { op: 'RestoreCursor', value: null } })
    expect(g.snapshot().cursor).toMatchObject({ row: 3, col: 5 })
  })

  it('RestoreCursor without a prior Save is a no-op', () => {
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    g.applyFrame({ frame: 'CursorOp', data: { op: 'Goto', value: { row: 2, col: 4 } } })
    // Never saved. Restore should leave cursor where it is.
    g.applyFrame({ frame: 'CursorOp', data: { op: 'RestoreCursor', value: null } })
    expect(g.snapshot().cursor).toMatchObject({ row: 1, col: 3 })
  })

  it('SetCursorVisible toggles cursor.visible flag', () => {
    // DECTCEM (CSI ?25 h/l). TUIs emit hide before a multi-step
    // repaint and show afterward so intermediate positions don't
    // flicker. The grid just mirrors the bit; the renderer honors
    // it by hiding the overlay.
    const g = new TerminalGrid({ rows: 5, cols: 10 })
    expect(g.snapshot().cursor.visible).toBe(true)
    g.applyFrame({ frame: 'CursorOp', data: { op: 'SetCursorVisible', value: false } })
    expect(g.snapshot().cursor.visible).toBe(false)
    g.applyFrame({ frame: 'CursorOp', data: { op: 'SetCursorVisible', value: true } })
    expect(g.snapshot().cursor.visible).toBe(true)
  })
})

// ── Semantic / raw frames are no-ops at the grid layer ──────────────

describe('TerminalGrid passthrough frames', () => {
  it('ignores SemanticEvent frames', () => {
    const g = new TerminalGrid({ rows: 2, cols: 5 })
    const beforeCursor = { ...g.snapshot().cursor }
    g.applyFrame({
      frame: 'SemanticEvent',
      data: { kind: { type: 'Message' }, payload: { text: 'hi' } },
    })
    expect(g.snapshot().cursor).toMatchObject(beforeCursor)
  })

  it('ignores AgentSignal frames', () => {
    const g = new TerminalGrid({ rows: 2, cols: 5 })
    g.applyFrame(text('abc'))
    g.applyFrame({
      frame: 'AgentSignal',
      data: {
        id: 'sig',
        from: { scope: 'agent', workspace: 'w', name: 'a' },
        to: { scope: 'agent', workspace: 'w', name: 'b' },
        kind: { kind: 'msg', data: { text: 'hi' } },
        at: '2026-04-21T00:00:00Z',
      },
    })
    expect(rowText(g, 0)).toBe('abc')
  })
})

// ── UTF-8 edge cases ─────────────────────────────────────────────────

describe('TerminalGrid UTF-8', () => {
  it('renders multi-byte UTF-8 correctly', () => {
    const g = new TerminalGrid({ rows: 2, cols: 10 })
    g.applyFrame(text('héllo'))
    expect(rowText(g, 0)).toBe('héllo')
  })

  it('buffers partial UTF-8 split across frames', () => {
    const g = new TerminalGrid({ rows: 2, cols: 10 })
    // 'é' = 0xC3 0xA9. Split between frames.
    g.applyFrame({ frame: 'Text', data: { bytes: [0x68, 0xc3], style: null } })
    g.applyFrame({ frame: 'Text', data: { bytes: [0xa9, 0x69], style: null } })
    expect(rowText(g, 0)).toBe('héi')
  })
})

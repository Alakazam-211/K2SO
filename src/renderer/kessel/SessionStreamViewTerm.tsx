// Canvas Plan Phase 5 — Kessel pane that renders from Tauri-side
// alacritty Term snapshots instead of the frontend TerminalGrid.
//
// Lifecycle:
//   1. On mount: invoke('kessel_term_attach', {...}) — Rust spawns
//      an async task that opens a WS to /cli/sessions/bytes and
//      drives bytes into an alacritty Term.
//   2. Listen for `kessel:grid-snapshot` events. Each event carries
//      a serialized Term snapshot (rows + scrollback as strings,
//      cursor, version).
//   3. Render the visible window from the snapshot, same DOM
//      shape as SessionStreamView (one <div> per row of <span>
//      spans).
//   4. Keyboard input: kessel_write (existing) forwards bytes to
//      the daemon PTY, which loops back via the byte stream into
//      our Term.
//   5. On window resize: kessel_term_resize calls term.resize
//      (which reflows scrollback at new cols natively — the
//      headline win of Phase 4 + 5).
//   6. On unmount: kessel_term_detach — Rust aborts the reader
//      task and drops the Term.
//
// Deliberately does NOT use the legacy TerminalGrid (grid.ts).
// Reflow, scrollback retention, cursor tracking are Term's job
// now. Selection, search, per-cell styling are Phase-6 polish.

import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useKesselConfig } from './config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
} from '@/lib/key-mapping'

export interface SessionStreamViewTermProps {
  /** Daemon SessionId (UUID). `null` = optimistic mount while the
   *  daemon spawn is in flight. */
  sessionId: string | null
  /** Daemon port — from kessel_spawn result. */
  port: number
  /** Auth token — from kessel_spawn result. */
  token: string
  /** Initial cols. Resize overrides this once ResizeObserver fires. */
  cols?: number
  /** Initial rows. Same as cols. */
  rows?: number
  /** Font size (px). */
  fontSize?: number
  /** Interactive? Default true. */
  interactive?: boolean
  /** Auto-resize via ResizeObserver? Default true. */
  autoResize?: boolean
}

interface TermCursor {
  row: number
  col: number
  visible: boolean
}

/** One run of consecutive cells in a row that share the same SGR
 *  style. Matches the Rust-side `CellRun` — keep these in lockstep
 *  with `src-tauri/src/commands/kessel_term.rs`. */
interface CellRun {
  text: string
  /** 0xRRGGBB or null (terminal default). */
  fg: number | null
  /** 0xRRGGBB or null (terminal default). */
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
}

interface TermGridSnapshot {
  paneId: string
  cols: number
  rows: number
  /** Each row is a list of style-homogeneous runs. */
  grid: CellRun[][]
  /** Scrollback, oldest-first. Same run-encoding as `grid`. */
  scrollback: CellRun[][]
  cursor: TermCursor
  version: number
  displayOffset: number
}

/** Delta update — only changed rows since last emit plus any new
 *  scrollback that accumulated between emits. Applied against the
 *  local grid mirror built up by a prior snapshot or delta chain.
 *  Matches the Rust-side `TermGridDelta`. */
interface DamagedRow {
  row: number
  runs: CellRun[]
}
interface TermGridDelta {
  paneId: string
  cols: number
  rows: number
  /** Rows in the live grid that changed since the last emit. */
  damagedRows: DamagedRow[]
  /** New rows that scrolled into scrollback between emits,
   *  oldest-first. Frontend appends to its scrollback buffer. */
  scrollbackAppended: CellRun[][]
  cursor: TermCursor
  version: number
  displayOffset: number
}

/** Convert a 24-bit hex color int to a CSS `rgb(...)` string. */
function hexToCss(n: number): string {
  const r = (n >> 16) & 0xff
  const g = (n >> 8) & 0xff
  const b = n & 0xff
  return `rgb(${r},${g},${b})`
}

/** Build a React CSS style for a run. Handles inverse, dim, and
 *  all the SGR flags that map directly to CSS. */
function runStyle(run: CellRun): React.CSSProperties {
  const fg = run.fg !== null ? hexToCss(run.fg) : undefined
  const bg = run.bg !== null ? hexToCss(run.bg) : undefined
  // SGR 7 (inverse): swap fg/bg. If either side is null (=
  // terminal default), the other side's color wins on its slot
  // and we let the inverted slot fall through to theme defaults.
  const color = run.inverse ? bg : fg
  const backgroundColor = run.inverse ? fg : bg
  const style: React.CSSProperties = {}
  if (color !== undefined) style.color = color
  if (backgroundColor !== undefined) style.backgroundColor = backgroundColor
  if (run.bold) style.fontWeight = 'bold'
  if (run.italic) style.fontStyle = 'italic'
  if (run.underline && run.strikeout) {
    style.textDecoration = 'underline line-through'
  } else if (run.underline) {
    style.textDecoration = 'underline'
  } else if (run.strikeout) {
    style.textDecoration = 'line-through'
  }
  if (run.dim) style.opacity = 0.6
  return style
}

/** Render a single row as a sequence of styled spans. Empty row
 *  (no runs) renders as a nbsp so the line is still selectable
 *  and retains its height. */
function renderRowRuns(row: CellRun[], rowIdx: number): React.ReactNode {
  if (row.length === 0) return '\u00a0'
  const spans: React.ReactNode[] = []
  for (let i = 0; i < row.length; i++) {
    const run = row[i]
    spans.push(
      <span key={`r${rowIdx}s${i}`} style={runStyle(run)}>
        {run.text || '\u00a0'}
      </span>,
    )
  }
  return spans
}

/** Stable pane id. Derived from sessionId but prefixed so the
 *  daemon side can tell Kessel pane requests from other pane
 *  types. */
function paneIdFor(sessionId: string): string {
  return `kessel-${sessionId}`
}

/** Phase 8 perf log: one-line structured entries on the browser
 *  console, consistent `[kessel-perf]` prefix so the user can
 *  copy-paste the output back. Aligned with the Rust-side
 *  `[kessel-perf] ts=... side=rust op=...` lines so a complete
 *  trace interleaves cleanly when timestamps are compared. */
function perfLog(op: string, fields: Record<string, unknown>): void {
  const parts = [`[kessel-perf]`, `ts=${Date.now()}`, `side=js`, `op=${op}`]
  for (const [k, v] of Object.entries(fields)) {
    if (v === undefined || v === null) continue
    const str =
      typeof v === 'string' && v.includes(' ') ? JSON.stringify(v) : String(v)
    parts.push(`${k}=${str}`)
  }
  // eslint-disable-next-line no-console
  console.info(parts.join(' '))
}

/** Apply a delta to a prior snapshot, returning the new snapshot
 *  that reflects the merged state. Returns `prev` unchanged if
 *  prev is null or its paneId doesn't match (frontend hasn't
 *  received its initial full snapshot yet, or the delta is for a
 *  different pane). Pure — no mutation of `prev`. */
function mergeDelta(
  prev: TermGridSnapshot | null,
  delta: TermGridDelta,
): TermGridSnapshot | null {
  if (!prev || prev.paneId !== delta.paneId) return prev
  // Rebuild the grid by copying prev.grid and overlaying damaged
  // rows at their indices. If the grid dimensions changed (rare
  // — usually after resize) rebuild to new shape:
  const nextGrid: CellRun[][] = prev.grid.slice()
  // If rows grew, pad with blanks. If rows shrank, truncate.
  while (nextGrid.length < delta.rows) nextGrid.push([])
  if (nextGrid.length > delta.rows) nextGrid.length = delta.rows
  for (const dr of delta.damagedRows) {
    if (dr.row < 0 || dr.row >= delta.rows) continue
    nextGrid[dr.row] = dr.runs
  }
  const nextScrollback: CellRun[][] =
    delta.scrollbackAppended.length > 0
      ? prev.scrollback.concat(delta.scrollbackAppended)
      : prev.scrollback
  return {
    paneId: prev.paneId,
    cols: delta.cols,
    rows: delta.rows,
    grid: nextGrid,
    scrollback: nextScrollback,
    cursor: delta.cursor,
    version: delta.version,
    displayOffset: delta.displayOffset,
  }
}

export function SessionStreamViewTerm(
  props: SessionStreamViewTermProps,
): React.JSX.Element {
  const config = useKesselConfig()
  const {
    sessionId,
    port,
    token,
    cols = 80,
    rows = 24,
    fontSize = config.font.size,
    interactive = true,
    autoResize = true,
  } = props

  const [snapshot, setSnapshot] = useState<TermGridSnapshot | null>(null)
  const [viewportOffset, setViewportOffset] = useState(0)
  const [isFocused, setIsFocused] = useState<boolean>(() =>
    typeof document !== 'undefined' ? document.hasFocus() : false,
  )

  const containerRef = useRef<HTMLDivElement>(null)
  const paneIdRef = useRef<string | null>(null)

  // ── Attach / detach lifecycle ─────────────────────────────────
  useEffect(() => {
    if (sessionId === null) return
    const paneId = paneIdFor(sessionId)
    paneIdRef.current = paneId
    let cancelled = false

    ;(async () => {
      const t0 = performance.now()
      try {
        await invoke('kessel_term_attach', {
          args: {
            paneId,
            sessionId,
            port,
            token,
            cols,
            rows,
          },
        })
        perfLog('attach_invoke', {
          pane: paneId,
          dur_ms: (performance.now() - t0).toFixed(2),
        })
      } catch (e) {
        if (!cancelled) {
          perfLog('attach_invoke', {
            pane: paneId,
            dur_ms: (performance.now() - t0).toFixed(2),
            error: String(e),
          })
        }
      }
    })()

    // Deliberately NO pullInitial call here. Panes mount in the
    // paused state, and the visibility effect below resumes them
    // only if they're in the visible tab. On first resume, the
    // reader emits a full snapshot — that's our initial state,
    // arriving via the event listener, never via a synchronous
    // invoke. This avoids the app-launch beachball where N
    // simultaneously-mounting panes fire N heavy synchronous
    // snapshot pulls concurrently through Tauri IPC.

    return () => {
      cancelled = true
      const t0 = performance.now()
      void invoke('kessel_term_detach', { paneId })
        .then(() => {
          perfLog('detach_invoke', {
            pane: paneId,
            dur_ms: (performance.now() - t0).toFixed(2),
          })
        })
        .catch(() => {})
      paneIdRef.current = null
    }
  }, [sessionId, port, token, cols, rows])

  // ── Snapshot + delta listeners ────────────────────────────────
  //
  // Full snapshots (`kessel:grid-snapshot`) land on first attach,
  // on resume after pause, and when the Term's damage says every
  // line changed. They replace the frontend's mirror wholesale.
  //
  // Deltas (`kessel:grid-delta`) are the steady-state hot path:
  // they carry only the row indices that changed + any new
  // scrollback rows that appeared. The frontend merges each
  // delta into its existing mirror.
  //
  // Ordering: we always install the listeners as pairs so a
  // delta arriving before its preceding snapshot (shouldn't
  // happen but guard anyway) gets dropped quietly.
  useEffect(() => {
    let unlistenSnap: UnlistenFn | null = null
    let unlistenDelta: UnlistenFn | null = null
    ;(async () => {
      unlistenSnap = await listen<TermGridSnapshot>(
        'kessel:grid-snapshot',
        (evt) => {
          // NOTE: this callback fires for EVERY pane's snapshot
          // (Tauri events are broadcast-all). The paneId filter
          // below throws out non-matching events — but the
          // callback *invocation* still costs main-thread time.
          // The perf log records every invocation, matched or
          // not, so the user can see the event fan-in cost.
          const ours = evt.payload.paneId === paneIdRef.current
          const t0 = performance.now()
          if (ours) {
            setSnapshot(evt.payload)
          }
          perfLog('rx_snapshot', {
            pane: evt.payload.paneId,
            ours,
            live_rows: evt.payload.grid.length,
            sb_rows: evt.payload.scrollback.length,
            dur_ms: (performance.now() - t0).toFixed(2),
          })
        },
      )
      unlistenDelta = await listen<TermGridDelta>(
        'kessel:grid-delta',
        (evt) => {
          const ours = evt.payload.paneId === paneIdRef.current
          const t0 = performance.now()
          if (ours) {
            setSnapshot((prev) => mergeDelta(prev, evt.payload))
          }
          perfLog('rx_delta', {
            pane: evt.payload.paneId,
            ours,
            damaged_rows: evt.payload.damagedRows.length,
            sb_append: evt.payload.scrollbackAppended.length,
            dur_ms: (performance.now() - t0).toFixed(2),
          })
        },
      )
    })()
    return () => {
      if (unlistenSnap) unlistenSnap()
      if (unlistenDelta) unlistenDelta()
    }
  }, [])

  // ── Focus tracking ────────────────────────────────────────────
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const on = (): void => setIsFocused(true)
    const off = (): void => setIsFocused(false)
    el.addEventListener('focus', on)
    el.addEventListener('blur', off)
    return () => {
      el.removeEventListener('focus', on)
      el.removeEventListener('blur', off)
    }
  }, [])

  // ── Auto-focus on tab activation ──────────────────────────────
  const isTabVisible = useIsTabVisible()
  useEffect(() => {
    if (!interactive) return
    if (!isTabVisible) return
    const el = containerRef.current
    if (!el) return
    const raf = requestAnimationFrame(() => {
      containerRef.current?.focus()
    })
    return () => cancelAnimationFrame(raf)
  }, [interactive, isTabVisible])

  // ── Hidden-pane emission pause ────────────────────────────────
  //
  // When the user switches away from a tab that contains a
  // Kessel pane, we tell the Tauri-side Term to stop emitting
  // snapshots/deltas to us. The Term keeps reading bytes and
  // advancing its state — so the session stays current — but
  // IPC + React state churn goes to zero. On becoming visible
  // again, resume triggers one full snapshot that catches us
  // back up in a single pass.
  //
  // Retained-view model means components stay mounted across
  // tab switches; without this gate, every hidden Kessel pane
  // would keep hammering the main thread with snapshot events
  // the user can't see. The workspace-switch lag the user
  // reported was exactly this symptom.
  useEffect(() => {
    const paneId = paneIdRef.current
    if (paneId === null) return
    const cmd = isTabVisible ? 'kessel_term_resume' : 'kessel_term_pause'
    const t0 = performance.now()
    invoke(cmd, { paneId })
      .then(() => {
        perfLog('visibility_invoke', {
          pane: paneId,
          cmd,
          visible: isTabVisible,
          dur_ms: (performance.now() - t0).toFixed(2),
        })
      })
      .catch(() => {
        /* pane may have detached mid-flight; ignore */
      })
  }, [isTabVisible])

  // ── Cell metrics (for cursor positioning + wheel math) ────────
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })
  useEffect(() => {
    const el = document.createElement('span')
    el.style.cssText = `font-family: ${config.font.family}; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    el.textContent = 'W'
    document.body.appendChild(el)
    const rect = el.getBoundingClientRect()
    document.body.removeChild(el)
    setCellMetrics({
      width: rect.width,
      height: Math.ceil(fontSize * config.font.lineHeightMultiplier),
    })
  }, [fontSize, config.font.family, config.font.lineHeightMultiplier])

  // ── Keyboard input ────────────────────────────────────────────
  useEffect(() => {
    if (!interactive) return
    if (sessionId === null) return
    const el = containerRef.current
    if (!el) return

    const send = (seq: string): void => {
      if (sessionId === null) return
      invoke('kessel_write', { sessionId, text: seq }).catch(() => {})
    }

    const onKey = (e: KeyboardEvent): void => {
      const natural = naturalTextEditingSequence(e)
      if (natural !== null) {
        e.preventDefault()
        setViewportOffset(0)
        send(natural)
        return
      }
      // Phase 5 first pass: default arrow-key mode (no app cursor
      // flag yet — snapshot doesn't carry Term modes). zsh/vim
      // users can still edit with CSI arrows; SS3 encoding will
      // land when we wire Term mode flags into the snapshot.
      const seq = keyEventToSequence(e, 0)
      if (seq === null) return
      e.preventDefault()
      setViewportOffset(0)
      send(seq)
    }
    const onPaste = (e: ClipboardEvent): void => {
      const text = e.clipboardData?.getData('text')
      if (!text) return
      e.preventDefault()
      setViewportOffset(0)
      send(text)
    }

    el.addEventListener('keydown', onKey)
    el.addEventListener('paste', onPaste)
    el.focus()
    return () => {
      el.removeEventListener('keydown', onKey)
      el.removeEventListener('paste', onPaste)
    }
  }, [interactive, sessionId])

  // ── ResizeObserver → kessel_term_resize + daemon /cli/sessions/resize ──
  useEffect(() => {
    if (!autoResize) return
    if (sessionId === null) return
    const el = containerRef.current
    if (!el) return
    if (!cellMetrics.width || !cellMetrics.height) return

    let timer: ReturnType<typeof setTimeout> | null = null
    let lastCols = cols
    let lastRows = rows
    const observer = new ResizeObserver((entries) => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        const rect = entries[0]?.contentRect
        if (!rect) return
        if (rect.width === 0 || rect.height === 0) return
        const availW = Math.max(0, rect.width - 8)
        const availH = Math.max(0, rect.height - 8)
        const newCols = Math.floor(availW / cellMetrics.width)
        const newRows = Math.floor(availH / cellMetrics.height)
        if (newCols < 10 || newRows < 3) return
        if (newCols === lastCols && newRows === lastRows) return
        lastCols = newCols
        lastRows = newRows
        const paneId = paneIdRef.current
        if (paneId) {
          invoke('kessel_term_resize', {
            paneId,
            cols: newCols,
            rows: newRows,
          }).catch(() => {})
        }
        // Also ask the daemon to SIGWINCH the PTY so Claude
        // redraws at the new size.
        if (sessionId !== null) {
          invoke('kessel_resize', {
            sessionId,
            cols: newCols,
            rows: newRows,
          }).catch(() => {})
        }
      }, 100)
    })
    observer.observe(el)
    return () => {
      if (timer) clearTimeout(timer)
      observer.disconnect()
    }
  }, [
    autoResize,
    cellMetrics.width,
    cellMetrics.height,
    sessionId,
    cols,
    rows,
  ])

  // ── Wheel scroll (client-side viewport offset over scrollback) ─
  //
  // For Phase 5 first pass, scrolling is purely a client-side
  // projection: we render different rows from the snapshot
  // without telling the Term. Same pattern SessionStreamView
  // uses. A future step can push scroll state into the Term
  // via a new kessel_term_scroll command if needed.
  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const FLUSH_MS = 50
    const onWheel = (e: WheelEvent): void => {
      if (e.deltaY === 0) return
      e.preventDefault()
      const cellH = cellMetrics.height || 20
      const pixelDelta =
        e.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? e.deltaY * cellH
          : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? e.deltaY * cellH * (snapshot?.rows ?? 24)
            : e.deltaY
      scrollAccumRef.current += pixelDelta
      if (!scrollTimerRef.current) {
        scrollTimerRef.current = setTimeout(() => {
          scrollTimerRef.current = null
          const accum = scrollAccumRef.current
          scrollAccumRef.current = 0
          if (accum === 0) return
          const lines = Math.round(
            (accum * config.scrolling.multiplier) / cellH,
          )
          if (lines === 0) return
          const maxOffset = snapshot?.scrollback.length ?? 0
          setViewportOffset((o) => {
            const next = o - lines
            if (next <= 0) return 0
            if (next >= maxOffset) return maxOffset
            return next
          })
        }, FLUSH_MS)
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      el.removeEventListener('wheel', onWheel)
      if (scrollTimerRef.current) {
        clearTimeout(scrollTimerRef.current)
        scrollTimerRef.current = null
      }
    }
  }, [config.scrolling.multiplier, cellMetrics.height, snapshot])

  // Drop DOM text selection on viewport change — matches
  // SessionStreamView's behavior until the proper content-space
  // selection overlay lands.
  useEffect(() => {
    if (typeof window === 'undefined') return
    const sel = window.getSelection()
    if (sel && sel.rangeCount > 0) sel.removeAllRanges()
  }, [viewportOffset])

  // ── Compose visible rows ──────────────────────────────────────
  const visibleRows = useMemo<CellRun[][]>(() => {
    if (!snapshot) return []
    if (viewportOffset === 0) return snapshot.grid
    const { scrollback, grid, rows: r } = snapshot
    const totalLen = scrollback.length + grid.length
    const windowEnd = totalLen - viewportOffset
    const windowStart = windowEnd - r
    const out: CellRun[][] = []
    for (let i = 0; i < r; i++) {
      const abs = windowStart + i
      if (abs < 0) out.push([])
      else if (abs < scrollback.length) out.push(scrollback[abs])
      else out.push(grid[abs - scrollback.length])
    }
    return out
  }, [viewportOffset, snapshot])

  // ── Container + cursor styles ─────────────────────────────────
  const containerStyle = useMemo<React.CSSProperties>(
    () => ({
      fontFamily: config.font.family,
      fontSize: `${fontSize}px`,
      lineHeight: `${Math.ceil(fontSize * config.font.lineHeightMultiplier)}px`,
      color: `rgb(${(config.colors.foreground >> 16) & 0xff},${(config.colors.foreground >> 8) & 0xff},${config.colors.foreground & 0xff})`,
      backgroundColor: `rgb(${(config.colors.background >> 16) & 0xff},${(config.colors.background >> 8) & 0xff},${config.colors.background & 0xff})`,
      whiteSpace: 'pre',
      padding: '4px',
      position: 'relative',
      overflow: 'hidden',
      width:
        !autoResize && cellMetrics.width
          ? `${cellMetrics.width * cols + 8}px`
          : '100%',
      height:
        !autoResize && cellMetrics.height
          ? `${cellMetrics.height * rows + 8}px`
          : '100%',
      flex: autoResize ? 1 : undefined,
    }),
    [
      fontSize,
      cellMetrics.width,
      cellMetrics.height,
      cols,
      rows,
      autoResize,
      config.font.family,
      config.font.lineHeightMultiplier,
      config.colors.foreground,
      config.colors.background,
    ],
  )

  const cursorStyle = useMemo<React.CSSProperties>(() => {
    if (!snapshot || !cellMetrics.width) return { display: 'none' }
    const cursorVisibleRow = snapshot.cursor.row + viewportOffset
    if (cursorVisibleRow < 0 || cursorVisibleRow >= snapshot.rows) {
      return { display: 'none' }
    }
    const caretColor = 'rgb(224, 224, 224)'
    const fill = isFocused ? caretColor : 'transparent'
    const outline = isFocused ? undefined : `inset 0 0 0 1px ${caretColor}`
    return {
      position: 'absolute',
      left: `${4 + cellMetrics.width * snapshot.cursor.col}px`,
      top: `${4 + cellMetrics.height * cursorVisibleRow}px`,
      width: `${cellMetrics.width}px`,
      height: `${cellMetrics.height}px`,
      backgroundColor: fill,
      boxShadow: outline,
      pointerEvents: 'none',
      boxSizing: 'border-box',
    }
  }, [snapshot, cellMetrics, viewportOffset, isFocused])

  return (
    <div
      ref={containerRef}
      className="kessel-session-stream-view-term"
      data-session-id={sessionId}
      tabIndex={interactive ? 0 : -1}
      style={{ ...containerStyle, outline: 'none' }}
    >
      {visibleRows.map((row, rowIdx) => (
        <div key={`row-${rowIdx}`}>{renderRowRuns(row, rowIdx)}</div>
      ))}
      <div aria-hidden="true" style={cursorStyle} />
      {import.meta.env.DEV && (
        <div
          style={{
            position: 'absolute',
            top: 2,
            right: 2,
            padding: '2px 6px',
            background: 'rgba(0,0,0,0.8)',
            color: '#ff0',
            fontSize: '10px',
            fontFamily: 'monospace',
            zIndex: 999,
            pointerEvents: 'none',
            borderRadius: '3px',
          }}
        >
          <strong style={{ color: '#fff' }}>Kessel Term</strong>{' '}
          · cells:{snapshot?.cols ?? '?'}x{snapshot?.rows ?? '?'}{' '}
          cursor:{snapshot?.cursor.col ?? 0},{snapshot?.cursor.row ?? 0}{' '}
          off:{viewportOffset}{' '}
          scr:{snapshot?.scrollback.length ?? 0}{' '}
          v:{snapshot?.version ?? 0}
        </div>
      )}
    </div>
  )
}

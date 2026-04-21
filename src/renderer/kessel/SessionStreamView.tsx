// Kessel — React component that renders a live Session Stream.
//
// Takes {sessionId, port, token} + optional dims, opens a KesselClient,
// feeds Frame events into a TerminalGrid, and projects the grid to
// DOM spans batched via requestAnimationFrame.
//
// Scope (I5): pure display. No keyboard input, no mouse, no resize
// handling (I6/I7/I8 add those). Input defaults to cols=80/rows=24
// unless props override. Visual parity with AlacrittyTerminalView
// is intentional — same font stack, same DEFAULT_FG/BG.

import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'

import { KesselClient } from './client'
import { TerminalGrid, type Cell, type GridSnapshot } from './grid'
import { DEFAULT_FG, DEFAULT_BG, styleToCss, stylesEqual } from './style'
import type { Frame } from './types'

// Font stack mirrors AlacrittyTerminalView so side-by-side users
// see the same glyphs. MesloLGM Nerd Font is bundled with K2SO.
const FONT_STACK =
  "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace"

export interface SessionStreamViewProps {
  /** SessionId UUID for the daemon's live session. */
  sessionId: string
  /** Daemon port — from `invoke('daemon_ws_url')`. */
  port: number
  /** Auth token — from `invoke('daemon_ws_url')`. */
  token: string
  /** Columns. Default 80. */
  cols?: number
  /** Rows. Default 24. */
  rows?: number
  /** Font size in px. Default 14. Typical terminal-settings store value. */
  fontSize?: number
  /** Fires once when the WS receives a session:ack (replay burst done). */
  onReady?: (replayCount: number) => void
  /** Fires on any WS / daemon error, including invalid JSON. */
  onError?: (message: string) => void
}

/** Render a single grid row as coalesced spans — adjacent cells with
 *  equal styles fuse into one `<span>` to keep the DOM tree bounded. */
function renderRow(row: readonly Cell[], rowIndex: number): React.ReactNode {
  if (row.length === 0) return null
  const spans: React.ReactNode[] = []
  let runStart = 0
  let runStyle = row[0].style
  let runText = row[0].char || ' '
  for (let i = 1; i < row.length; i++) {
    const cell = row[i]
    if (stylesEqual(cell.style, runStyle)) {
      runText += cell.char || ' '
      continue
    }
    spans.push(
      <span key={`r${rowIndex}c${runStart}`} style={styleToCss(runStyle)}>
        {runText}
      </span>,
    )
    runStart = i
    runStyle = cell.style
    runText = cell.char || ' '
  }
  spans.push(
    <span key={`r${rowIndex}c${runStart}`} style={styleToCss(runStyle)}>
      {runText}
    </span>,
  )
  return spans
}

export function SessionStreamView(props: SessionStreamViewProps): React.JSX.Element {
  const {
    sessionId,
    port,
    token,
    cols = 80,
    rows = 24,
    fontSize = 14,
    onReady,
    onError,
  } = props

  // TerminalGrid is held in a ref because it's imperative state —
  // Frame events mutate it in place, and we trigger a React rerender
  // via a version counter each animation frame.
  const gridRef = useRef<TerminalGrid | null>(null)
  if (gridRef.current === null) {
    gridRef.current = new TerminalGrid({ cols, rows })
  }

  const [snapshot, setSnapshot] = useState<GridSnapshot>(() =>
    gridRef.current!.snapshot(),
  )
  const rafPendingRef = useRef(false)
  const scheduleRender = useCallback(() => {
    if (rafPendingRef.current) return
    rafPendingRef.current = true
    requestAnimationFrame(() => {
      rafPendingRef.current = false
      setSnapshot(gridRef.current!.snapshot())
    })
  }, [])

  // Propagate prop-driven resize into the grid.
  useEffect(() => {
    if (!gridRef.current) return
    if (gridRef.current.rows !== rows || gridRef.current.cols !== cols) {
      gridRef.current.resize(cols, rows)
      scheduleRender()
    }
  }, [cols, rows, scheduleRender])

  // Open the WS once per (sessionId, port, token) tuple. dispose on
  // unmount or prop change.
  useEffect(() => {
    const client = new KesselClient({ sessionId, port, token })
    const off = client.on({
      onFrame: (frame: Frame) => {
        gridRef.current!.applyFrame(frame)
        scheduleRender()
      },
      onAck: (ack) => onReady?.(ack.replayCount),
      onError: (err) => onError?.(err.message),
    })
    client.connect()
    return () => {
      off()
      client.dispose()
    }
  }, [sessionId, port, token, onReady, onError, scheduleRender])

  // Cell metrics for cursor positioning. Measured once per fontSize
  // change by writing a hidden span and reading its box. Simple and
  // accurate; matches AlacrittyTerminalView's approach.
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })
  useEffect(() => {
    const el = document.createElement('span')
    el.style.cssText = `font-family: ${FONT_STACK}; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    el.textContent = 'W'
    document.body.appendChild(el)
    const rect = el.getBoundingClientRect()
    document.body.removeChild(el)
    setCellMetrics({
      width: rect.width,
      height: Math.ceil(fontSize * 1.2),
    })
  }, [fontSize])

  const containerStyle = useMemo<React.CSSProperties>(
    () => ({
      fontFamily: FONT_STACK,
      fontSize: `${fontSize}px`,
      lineHeight: `${Math.ceil(fontSize * 1.2)}px`,
      color: `rgb(${(DEFAULT_FG >> 16) & 0xff},${(DEFAULT_FG >> 8) & 0xff},${DEFAULT_FG & 0xff})`,
      backgroundColor: `rgb(${(DEFAULT_BG >> 16) & 0xff},${(DEFAULT_BG >> 8) & 0xff},${DEFAULT_BG & 0xff})`,
      whiteSpace: 'pre',
      padding: '4px',
      position: 'relative',
      overflow: 'hidden',
      // Deterministic width/height so a resize that hasn't yet
      // reached the grid still clamps the view to the requested
      // dims — no flicker while resize propagates through props.
      width: cellMetrics.width
        ? `${cellMetrics.width * cols + 8}px`
        : undefined,
      height: cellMetrics.height
        ? `${cellMetrics.height * rows + 8}px`
        : undefined,
    }),
    [fontSize, cellMetrics.width, cellMetrics.height, cols, rows],
  )

  const cursorStyle = useMemo<React.CSSProperties>(() => {
    if (!snapshot.cursor.visible) return { display: 'none' }
    if (!cellMetrics.width) return { display: 'none' }
    return {
      position: 'absolute',
      left: `${4 + cellMetrics.width * snapshot.cursor.col}px`,
      top: `${4 + cellMetrics.height * snapshot.cursor.row}px`,
      width: `${cellMetrics.width}px`,
      height: `${cellMetrics.height}px`,
      backgroundColor: 'rgba(224, 224, 224, 0.5)',
      pointerEvents: 'none',
    }
  }, [
    snapshot.cursor.visible,
    snapshot.cursor.col,
    snapshot.cursor.row,
    cellMetrics.width,
    cellMetrics.height,
  ])

  return (
    <div
      className="kessel-session-stream-view"
      data-session-id={sessionId}
      style={containerStyle}
    >
      {snapshot.grid.map((row, rowIdx) => (
        <div key={`row-${rowIdx}`}>{renderRow(row, rowIdx)}</div>
      ))}
      <div aria-hidden="true" style={cursorStyle} />
    </div>
  )
}

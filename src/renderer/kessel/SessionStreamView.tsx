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
import { keyEventToSequence } from '@/lib/key-mapping'

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
  /** If true, the view grabs focus on mount and keydown bytes stream
   *  to the daemon's /cli/terminal/write. Default true. Set false
   *  for debug / read-only viewers. */
  interactive?: boolean
  /** If true, the pane observes its own DOM size and auto-resizes
   *  the grid + PTY to fit. Default true. Set false when the parent
   *  controls sizing explicitly (e.g. the Harness Lab's device-
   *  size preset buttons). */
  autoResize?: boolean
}

/** POST bytes to /cli/terminal/write for the given session. Returns
 *  a promise so callers can decide whether to await (the happy path
 *  is fire-and-forget — we don't block keystrokes on HTTP round-trip).
 *  Bytes are URL-encoded; for binary non-UTF-8 sequences, the write
 *  endpoint accepts raw UTF-8 text (key-mapping's escape sequences
 *  are ASCII so this is safe). */
async function writeToSession(
  port: number,
  token: string,
  sessionId: string,
  text: string,
): Promise<void> {
  const params = new URLSearchParams({
    id: sessionId,
    message: text,
    token,
    no_submit: 'true', // we send Enter explicitly via key-mapping
  })
  const url = `http://127.0.0.1:${port}/cli/terminal/write?${params}`
  await fetch(url, { method: 'GET' })
}

/** POST to /cli/sessions/resize (I7). Fire-and-forget — the grid
 *  updates its own dimensions locally; the daemon call just keeps
 *  the child process in sync. */
async function resizeSession(
  port: number,
  token: string,
  sessionId: string,
  cols: number,
  rows: number,
): Promise<void> {
  const params = new URLSearchParams({
    session: sessionId,
    cols: String(cols),
    rows: String(rows),
    token,
  })
  const url = `http://127.0.0.1:${port}/cli/sessions/resize?${params}`
  await fetch(url, { method: 'GET' })
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
    interactive = true,
    autoResize = true,
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

  // Cursor blink. The snapshot carries an up-to-the-millisecond
  // cursor position (mutated by every Frame::Text write + CursorOp),
  // but rendering at that cadence makes Claude's rapid cursor moves
  // look like the caret is "vibrating." Real terminals blink the
  // cursor at ~500ms regardless of input traffic, so the eye
  // perceives it as stable. We do the same here: a 500ms interval
  // flips this boolean; the cursor overlay hides during the off
  // phase. Any user keystroke resets the phase to ON so typing
  // feels instant.
  const [cursorBlinkOn, setCursorBlinkOn] = useState(true)
  useEffect(() => {
    const id = setInterval(() => setCursorBlinkOn((v) => !v), 500)
    return () => clearInterval(id)
  }, [])

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

  // Keyboard input path (I6). Keydown → key-mapping encoder →
  // daemon's /cli/terminal/write. Focus is grabbed on mount when
  // interactive=true so the pane is keyboard-reachable without the
  // user having to click first.
  const containerRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!interactive) return
    const el = containerRef.current
    if (!el) return
    const handler = (e: KeyboardEvent) => {
      // Key-mapping doesn't know our mode flags yet (Phase 2
      // LineMux doesn't emit them). Default mode = 0; shell
      // apps that need APP_CURSOR for arrow keys will degrade
      // gracefully (still functional, just uses raw ESC[A etc.).
      const seq = keyEventToSequence(e, 0)
      if (seq === null) return
      e.preventDefault()
      // Any keystroke forces the cursor to the ON phase of the
      // blink cycle so typing feels snappy (same UX as a real
      // terminal). The 500ms interval will resume from ON.
      setCursorBlinkOn(true)
      // Fire-and-forget; network-bound latency is not in the
      // render path. Errors log to console for now; a future
      // commit can route them to the onError prop.
      writeToSession(port, token, sessionId, seq).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('[kessel] write failed:', err)
      })
    }
    // I8 — paste handler. Browser DOM gives us selection + copy
    // (Cmd+C) for free since the pane is native <span>s; the gap
    // was paste, since the pane isn't an editable input. Listen for
    // the 'paste' event (fires on Cmd+V when focused) and forward
    // the clipboard text through /cli/terminal/write. The shell /
    // claude / whoever receives exactly what the user pasted.
    //
    // Known follow-up: when a receiving program has bracketed-paste
    // mode enabled (CSI ?2004h), pasted text should be wrapped in
    // ESC[200~ ... ESC[201~. LineMux doesn't surface this mode yet
    // — see the roadmap. For now raw-pastes work correctly for
    // bash/zsh/claude; multi-line pastes into a line-oriented shell
    // will execute each line (same as any unwrapped paste).
    const pasteHandler = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData('text')
      if (!text) return
      e.preventDefault()
      setCursorBlinkOn(true)
      writeToSession(port, token, sessionId, text).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('[kessel] paste write failed:', err)
      })
    }
    el.addEventListener('keydown', handler)
    el.addEventListener('paste', pasteHandler)
    // Grab focus so the user can type immediately.
    el.focus()
    return () => {
      el.removeEventListener('keydown', handler)
      el.removeEventListener('paste', pasteHandler)
    }
  }, [interactive, port, token, sessionId])

  // I7 — ResizeObserver on the pane container. On dimension change,
  // compute new cols/rows from cell metrics + container box, resize
  // the TerminalGrid locally, and POST to /cli/sessions/resize so
  // the child process sees the new dimensions.
  //
  // Debounce ~100ms: drag-resize fires many events per second;
  // batching keeps network + grid churn bounded while still feeling
  // live.
  useEffect(() => {
    if (!autoResize) return
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
        // Subtract the 4px padding on each side.
        const availW = Math.max(0, rect.width - 8)
        const availH = Math.max(0, rect.height - 8)
        // Safety floor: if we'd compute < MIN_VIEWPORT cols or rows,
        // refuse to resize the grid. A zero-width measurement (CSS
        // layout not settled yet) would otherwise collapse the grid
        // to 1×1 and every byte would wrap to a new row.
        const MIN_COLS = 10
        const MIN_ROWS = 3
        const newCols = Math.max(MIN_COLS, Math.floor(availW / cellMetrics.width))
        const newRows = Math.max(MIN_ROWS, Math.floor(availH / cellMetrics.height))
        if (newCols === lastCols && newRows === lastRows) return
        lastCols = newCols
        lastRows = newRows
        gridRef.current!.resize(newCols, newRows)
        scheduleRender()
        resizeSession(port, token, sessionId, newCols, newRows).catch(
          (err) => {
            // eslint-disable-next-line no-console
            console.warn('[kessel] resize failed:', err)
          },
        )
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
    port,
    token,
    sessionId,
    cols,
    rows,
    scheduleRender,
  ])

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
      // When autoResize is off, pin the pane to the requested
      // dims so a prop-driven resize clamps the view immediately
      // and doesn't flicker while the grid catches up.
      //
      // When autoResize is on, defer sizing to the parent — the
      // pane takes whatever box it's given and the
      // ResizeObserver computes cols/rows to fit. flex:1 + width
      // 100% + height 100% so the pane fills a flex container
      // (Harness Lab, future tab panes). Without this, a flex-row
      // parent would collapse the pane to 1 col because a block-
      // level div has no intrinsic width beyond its content's.
      width: !autoResize && cellMetrics.width
        ? `${cellMetrics.width * cols + 8}px`
        : '100%',
      height: !autoResize && cellMetrics.height
        ? `${cellMetrics.height * rows + 8}px`
        : '100%',
      flex: autoResize ? 1 : undefined,
    }),
    [fontSize, cellMetrics.width, cellMetrics.height, cols, rows, autoResize],
  )

  const cursorStyle = useMemo<React.CSSProperties>(() => {
    if (!snapshot.cursor.visible || !cursorBlinkOn) return { display: 'none' }
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
    cursorBlinkOn,
  ])

  return (
    <div
      ref={containerRef}
      className="kessel-session-stream-view"
      data-session-id={sessionId}
      tabIndex={interactive ? 0 : -1}
      style={{ ...containerStyle, outline: 'none' }}
    >
      {snapshot.grid.map((row, rowIdx) => (
        <div key={`row-${rowIdx}`}>{renderRow(row, rowIdx)}</div>
      ))}
      <div aria-hidden="true" style={cursorStyle} />
    </div>
  )
}

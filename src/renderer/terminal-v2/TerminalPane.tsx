// Alacritty_v2 Tauri thin client.
//
// Speaks the A3/A4 protocol defined in
// `.k2so/prds/alacritty-v2.md`:
//
//   1. POST /cli/sessions/v2/spawn with {agent_name, cwd, ...}
//      → {sessionId, agentName, cols, rows, reused}.
//   2. Open WS to /cli/sessions/grid?session=<uuid>&token=<token>.
//   3. Receive {event:"snapshot", payload:TermGridSnapshot} first,
//      then stream of {event:"delta", payload:TermGridDelta}.
//   4. On keystroke / paste: send {action:"input", text}.
//   5. On ResizeObserver: send {action:"resize", cols, rows}.
//   6. On unmount: close WS socket only. Session survives on
//      daemon — v2's whole point. Explicit close happens via
//      /cli/sessions/v2/close from tabs.ts removeTab (A6).
//
// No local alacritty_terminal::Term. No ANSI parser. No byte
// stream. The daemon does all of that; we render JSON-serialized
// grid deltas to DOM using the CellRun vocabulary from
// k2so-core's grid_snapshot module.
//
// Deliberately kept small (< 450 lines). The Kessel-era
// SessionStreamViewTerm was ~600 lines because it held a local
// Term + byte reader + APC filter. None of that here.

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'

import { useKesselConfig } from '../kessel/config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
} from '@/lib/key-mapping'
import { getDaemonWs, invalidateDaemonWs } from '../kessel/daemon-ws'

// ── Wire types (mirror k2so-core/src/terminal/grid_snapshot.rs) ───

interface CellRun {
  text: string
  fg: number | null
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
}

interface CursorSnapshot {
  row: number
  col: number
  visible: boolean
}

interface TermGridSnapshot {
  paneId: string
  cols: number
  rows: number
  grid: CellRun[][]
  scrollback: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
}

interface DamagedRow {
  row: number
  runs: CellRun[]
}

interface TermGridDelta {
  paneId: string
  cols: number
  rows: number
  damagedRows: DamagedRow[]
  scrollbackAppended: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
}

type OutboundMsg =
  | { event: 'snapshot'; payload: TermGridSnapshot }
  | { event: 'delta'; payload: TermGridDelta }
  | { event: 'child_exit'; payload: { exit_code: number | null } }
  | { event: 'error'; payload: { message: string } }

// ── Helpers ───────────────────────────────────────────────────────

function hexToCss(n: number): string {
  const r = (n >> 16) & 0xff
  const g = (n >> 8) & 0xff
  const b = n & 0xff
  return `rgb(${r},${g},${b})`
}

function runStyle(run: CellRun): React.CSSProperties {
  const fg = run.fg !== null ? hexToCss(run.fg) : undefined
  const bg = run.bg !== null ? hexToCss(run.bg) : undefined
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

/** Merge a delta into a prior snapshot. Pure. Returns `prev`
 *  unchanged if no prior snapshot exists yet (delta arrived
 *  before the initial snapshot — shouldn't happen per protocol,
 *  but guard anyway). */
function mergeDelta(
  prev: TermGridSnapshot | null,
  delta: TermGridDelta,
): TermGridSnapshot | null {
  if (!prev) return prev
  const nextGrid: CellRun[][] = prev.grid.slice()
  while (nextGrid.length < delta.rows) nextGrid.push([])
  if (nextGrid.length > delta.rows) nextGrid.length = delta.rows
  for (const dr of delta.damagedRows) {
    if (dr.row < 0 || dr.row >= delta.rows) continue
    nextGrid[dr.row] = dr.runs
  }
  const nextScrollback =
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

// ── Component ─────────────────────────────────────────────────────

export interface TerminalPaneProps {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  fontSize?: number
  spawnedAt?: number
}

type Phase =
  | { kind: 'idle' }
  | { kind: 'spawning' }
  | { kind: 'connecting'; sessionId: string }
  | { kind: 'ready'; sessionId: string }
  | { kind: 'exited'; sessionId: string; exitCode: number | null }
  | { kind: 'error'; message: string }

export function TerminalPane(props: TerminalPaneProps): React.JSX.Element {
  const config = useKesselConfig()
  const { terminalId, cwd, command, args, fontSize = config.font.size } = props

  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const [snapshot, setSnapshot] = useState<TermGridSnapshot | null>(null)
  const [viewportOffset, setViewportOffset] = useState(0)
  const [isFocused, setIsFocused] = useState<boolean>(() =>
    typeof document !== 'undefined' ? document.hasFocus() : false,
  )

  const containerRef = useRef<HTMLDivElement>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const isTabVisible = useIsTabVisible()

  // ── Spawn + WS lifecycle ──────────────────────────────────────
  //
  // One effect handles the whole flow: HTTP POST to v2 spawn, then
  // open WS. Any step failing parks the component in `{error}` and
  // surfaces a message overlay. Cleanup on unmount closes the WS
  // only — daemon-side session survives.
  useEffect(() => {
    let cancelled = false
    const agentName = `tab-${terminalId}`

    async function boot() {
      setPhase({ kind: 'spawning' })
      let creds: { port: number; token: string }
      try {
        creds = await getDaemonWs()
      } catch (e) {
        if (!cancelled) setPhase({ kind: 'error', message: `daemon unreachable: ${String(e)}` })
        return
      }

      const spawnBody = {
        agent_name: agentName,
        cwd,
        command: command ?? null,
        args: args ?? null,
        // Default cols/rows matter little — ResizeObserver corrects
        // via a /cli/sessions/v2/spawn-time value AND a follow-up
        // resize message once we measure the container.
        cols: 120,
        rows: 40,
      }
      let spawnRes: Response
      try {
        spawnRes = await fetch(
          `http://127.0.0.1:${creds.port}/cli/sessions/v2/spawn?token=${creds.token}`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(spawnBody),
          },
        )
      } catch (e) {
        invalidateDaemonWs()
        if (!cancelled) setPhase({ kind: 'error', message: `spawn fetch failed: ${String(e)}` })
        return
      }
      if (!spawnRes.ok) {
        const body = await spawnRes.text()
        if (!cancelled) setPhase({ kind: 'error', message: `spawn ${spawnRes.status}: ${body}` })
        return
      }
      const spawn = (await spawnRes.json()) as {
        sessionId: string
        agentName: string
        cols: number
        rows: number
        reused: boolean
      }
      if (cancelled) return

      setPhase({ kind: 'connecting', sessionId: spawn.sessionId })

      const ws = new WebSocket(
        `ws://127.0.0.1:${creds.port}/cli/sessions/grid?session=${spawn.sessionId}&token=${creds.token}`,
      )
      wsRef.current = ws

      ws.onmessage = (evt) => {
        if (typeof evt.data !== 'string') return
        let parsed: OutboundMsg
        try {
          parsed = JSON.parse(evt.data) as OutboundMsg
        } catch {
          return
        }
        switch (parsed.event) {
          case 'snapshot':
            setSnapshot(parsed.payload)
            setPhase({ kind: 'ready', sessionId: spawn.sessionId })
            break
          case 'delta':
            setSnapshot((prev) => mergeDelta(prev, parsed.payload))
            break
          case 'child_exit':
            setPhase({
              kind: 'exited',
              sessionId: spawn.sessionId,
              exitCode: parsed.payload.exit_code,
            })
            break
          case 'error':
            setPhase({ kind: 'error', message: parsed.payload.message })
            break
        }
      }

      ws.onerror = () => {
        if (!cancelled) setPhase({ kind: 'error', message: 'ws error' })
      }
      ws.onclose = () => {
        // Clean client-side state. Session on daemon is unaffected
        // unless the daemon itself closed (child exit handled above).
      }
    }

    void boot()

    return () => {
      cancelled = true
      // Close the WS but do NOT call /cli/sessions/v2/close.
      // Daemon session survives. Deliberate tab-close teardown
      // is wired in A6 via tabs.ts::removeTab.
      const ws = wsRef.current
      if (ws && ws.readyState !== WebSocket.CLOSED) {
        ws.close()
      }
      wsRef.current = null
    }
  }, [terminalId, cwd, command, args?.join('\0')])

  // ── Focus tracking ────────────────────────────────────────────
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const on = () => setIsFocused(true)
    const off = () => setIsFocused(false)
    el.addEventListener('focus', on)
    el.addEventListener('blur', off)
    return () => {
      el.removeEventListener('focus', on)
      el.removeEventListener('blur', off)
    }
  }, [])

  // Auto-focus when tab becomes visible.
  useEffect(() => {
    if (!isTabVisible) return
    const el = containerRef.current
    if (!el) return
    const raf = requestAnimationFrame(() => el.focus())
    return () => cancelAnimationFrame(raf)
  }, [isTabVisible])

  // ── Cell metrics (for cursor positioning + wheel math) ────────
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })
  useLayoutEffect(() => {
    const span = document.createElement('span')
    span.style.cssText = `font-family: ${config.font.family}; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    span.textContent = 'W'
    document.body.appendChild(span)
    const rect = span.getBoundingClientRect()
    document.body.removeChild(span)
    setCellMetrics({
      width: rect.width,
      height: Math.ceil(fontSize * config.font.lineHeightMultiplier),
    })
  }, [fontSize, config.font.family, config.font.lineHeightMultiplier])

  // ── Send input / resize ───────────────────────────────────────
  const sendInput = useCallback((text: string) => {
    const ws = wsRef.current
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    ws.send(JSON.stringify({ action: 'input', text }))
  }, [])

  const sendResize = useCallback((cols: number, rows: number) => {
    const ws = wsRef.current
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    ws.send(JSON.stringify({ action: 'resize', cols, rows }))
  }, [])

  // ── Keyboard input ────────────────────────────────────────────
  useEffect(() => {
    if (phase.kind !== 'ready') return
    const el = containerRef.current
    if (!el) return

    const onKey = (e: KeyboardEvent) => {
      const natural = naturalTextEditingSequence(e)
      if (natural !== null) {
        e.preventDefault()
        setViewportOffset(0)
        sendInput(natural)
        return
      }
      const seq = keyEventToSequence(e, 0)
      if (seq === null) return
      e.preventDefault()
      setViewportOffset(0)
      sendInput(seq)
    }
    const onPaste = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData('text')
      if (!text) return
      e.preventDefault()
      setViewportOffset(0)
      sendInput(text)
    }

    el.addEventListener('keydown', onKey)
    el.addEventListener('paste', onPaste)
    el.focus()
    return () => {
      el.removeEventListener('keydown', onKey)
      el.removeEventListener('paste', onPaste)
    }
  }, [phase.kind, sendInput])

  // ── ResizeObserver → send resize ──────────────────────────────
  useEffect(() => {
    if (phase.kind !== 'ready') return
    const el = containerRef.current
    if (!el) return
    if (!cellMetrics.width || !cellMetrics.height) return

    let lastCols = 0
    let lastRows = 0
    let timer: ReturnType<typeof setTimeout> | null = null
    const observer = new ResizeObserver((entries) => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        const rect = entries[0]?.contentRect
        if (!rect || rect.width === 0 || rect.height === 0) return
        const availW = Math.max(0, rect.width - 8)
        const availH = Math.max(0, rect.height - 8)
        const newCols = Math.floor(availW / cellMetrics.width)
        const newRows = Math.floor(availH / cellMetrics.height)
        if (newCols < 10 || newRows < 3) return
        if (newCols === lastCols && newRows === lastRows) return
        lastCols = newCols
        lastRows = newRows
        sendResize(newCols, newRows)
      }, 100)
    })
    observer.observe(el)
    return () => {
      if (timer) clearTimeout(timer)
      observer.disconnect()
    }
  }, [phase.kind, cellMetrics.width, cellMetrics.height, sendResize])

  // ── Wheel scroll (client-side viewport offset) ────────────────
  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const FLUSH_MS = 50
    const onWheel = (e: WheelEvent) => {
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

  // ── Styles ────────────────────────────────────────────────────
  const containerStyle: React.CSSProperties = useMemo(
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
      flex: 1,
      width: '100%',
      height: '100%',
      outline: 'none',
    }),
    [
      fontSize,
      config.font.family,
      config.font.lineHeightMultiplier,
      config.colors.foreground,
      config.colors.background,
    ],
  )

  const cursorStyle: React.CSSProperties = useMemo(() => {
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

  // ── Render ────────────────────────────────────────────────────
  if (phase.kind === 'error') {
    return (
      <div
        style={{
          padding: 16,
          color: '#ff6666',
          fontFamily: 'monospace',
          fontSize: 12,
          whiteSpace: 'pre-wrap',
        }}
      >
        Alacritty v2: {phase.message}
      </div>
    )
  }

  const isReady = phase.kind === 'ready' || phase.kind === 'exited'
  const debugSessionId =
    phase.kind === 'ready' || phase.kind === 'connecting' || phase.kind === 'exited'
      ? phase.sessionId
      : null

  return (
    <div
      ref={containerRef}
      className="alacritty-v2-pane"
      data-session-id={debugSessionId}
      tabIndex={0}
      style={containerStyle}
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
          <strong style={{ color: '#fff' }}>Alacritty</strong>
          {' '}· phase:{phase.kind}
          {' '}cells:{snapshot?.cols ?? '?'}x{snapshot?.rows ?? '?'}
          {' '}cursor:{snapshot?.cursor.col ?? 0},{snapshot?.cursor.row ?? 0}
          {' '}off:{viewportOffset}
          {' '}scr:{snapshot?.scrollback.length ?? 0}
          {' '}v:{snapshot?.version ?? 0}
          {!isReady && phase.kind !== 'idle' && ' · loading'}
        </div>
      )}
    </div>
  )
}

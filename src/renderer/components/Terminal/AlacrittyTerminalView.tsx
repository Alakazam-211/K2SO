import { useEffect, useRef, useCallback, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useSettingsStore } from '@/stores/settings'
import { keyEventToSequence, naturalTextEditingSequence } from '@/lib/key-mapping'

// ── Types matching Rust GridUpdate / CompactLine / StyleSpan ──────────

interface StyleSpan {
  s: number  // start column (inclusive)
  e: number  // end column (inclusive)
  fg?: number  // foreground 0xRRGGBB
  bg?: number  // background 0xRRGGBB
  fl?: number  // flags bitfield
}

interface CompactLine {
  row: number
  text: string
  spans?: StyleSpan[]
}

interface GridUpdate {
  cols: number
  rows: number
  cursor_col: number
  cursor_row: number
  cursor_visible: boolean
  cursor_shape: string
  lines: CompactLine[]
  full: boolean
  mode: number
  display_offset: number
  selection?: [number, number, number, number]
}

// Attribute flags (must match Rust ATTR_* constants)
const ATTR_BOLD = 1
const ATTR_ITALIC = 2
const ATTR_UNDERLINE = 4
const ATTR_STRIKETHROUGH = 8
const ATTR_INVERSE = 16
const ATTR_DIM = 32
const ATTR_HIDDEN = 64

const DEFAULT_FG = 0xe0e0e0
const DEFAULT_BG = 0x0a0a0a

// ── Props ────────────────────────────────────────────────────────────

interface AlacrittyTerminalViewProps {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  onExit?: (exitCode: number) => void
}

function shellEscape(path: string): string {
  return "'" + path.replace(/'/g, "'\\''") + "'"
}

function colorToCSS(c: number): string {
  return `rgb(${(c >> 16) & 0xff},${(c >> 8) & 0xff},${c & 0xff})`
}

// ── Line Renderer ────────────────────────────────────────────────────

function renderLineSpans(line: CompactLine): React.JSX.Element[] {
  const { text, spans } = line
  if (!text) return [<span key="empty">{'\u00A0'}</span>]
  if (!spans || spans.length === 0) return [<span key="default">{text || '\u00A0'}</span>]

  const elements: React.JSX.Element[] = []
  let lastEnd = 0

  // Convert character indices to string positions
  // CompactLine text has trailing spaces trimmed, spans reference column indices
  const chars = [...text]  // handle multi-byte chars correctly

  for (let i = 0; i < spans.length; i++) {
    const span = spans[i]

    // Add unstyled text before this span
    if (span.s > lastEnd) {
      const unstyled = chars.slice(lastEnd, span.s).join('')
      if (unstyled) elements.push(<span key={`g${i}`}>{unstyled}</span>)
    }

    // Build style for this span
    const style: React.CSSProperties = {}
    const flags = span.fl ?? 0
    let fg = span.fg ?? DEFAULT_FG
    let bg = span.bg ?? DEFAULT_BG

    if (flags & ATTR_INVERSE) {
      const tmp = fg; fg = bg; bg = tmp
    }

    if (fg !== DEFAULT_FG) style.color = colorToCSS(fg)
    if (bg !== DEFAULT_BG) style.backgroundColor = colorToCSS(bg)
    if (flags & ATTR_BOLD) style.fontWeight = 'bold'
    if (flags & ATTR_ITALIC) style.fontStyle = 'italic'
    if (flags & ATTR_DIM) style.opacity = 0.7
    if (flags & ATTR_HIDDEN) style.color = 'transparent'
    if (flags & ATTR_UNDERLINE) style.textDecoration = 'underline'
    if (flags & ATTR_STRIKETHROUGH) {
      style.textDecoration = style.textDecoration
        ? `${style.textDecoration} line-through`
        : 'line-through'
    }

    const styledText = chars.slice(span.s, span.e + 1).join('')
    elements.push(
      <span key={`s${i}`} style={Object.keys(style).length > 0 ? style : undefined}>
        {styledText}
      </span>
    )

    lastEnd = span.e + 1
  }

  // Add remaining unstyled text after last span
  if (lastEnd < chars.length) {
    elements.push(<span key="tail">{chars.slice(lastEnd).join('')}</span>)
  }

  return elements
}

// ── Component ────────────────────────────────────────────────────────

export function AlacrittyTerminalView({
  terminalId,
  cwd,
  command,
  args,
  onExit,
}: AlacrittyTerminalViewProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  const ptyIdRef = useRef<string | null>(null)
  const cellMetricsRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 })
  const [cellMetrics, setCellMetrics] = useState<{ width: number; height: number }>({ width: 0, height: 0 })
  const lastColsRef = useRef(0)
  const lastRowsRef = useRef(0)
  const termModeRef = useRef(0)
  const rafPendingRef = useRef(false)
  const pendingFrameRef = useRef<GridUpdate | null>(null)

  const fontSize = useTerminalSettingsStore((s) => s.fontSize)
  const settings = useSettingsStore((s) => s.settings)
  const naturalTextEditing = settings?.terminal?.naturalTextEditing !== false

  const [created, setCreated] = useState(false)
  const [cursorBlinkVisible, setCursorBlinkVisible] = useState(true)

  // Grid state
  const linesRef = useRef<Map<number, CompactLine>>(new Map())
  const [gridState, setGridState] = useState<{
    rows: number
    cols: number
    cursorCol: number
    cursorRow: number
    cursorVisible: boolean
    cursorShape: string
    displayOffset: number
    version: number  // increment to trigger re-render
  }>({ rows: 24, cols: 80, cursorCol: 0, cursorRow: 0, cursorVisible: true, cursorShape: 'bar', displayOffset: 0, version: 0 })

  // Debug
  const frameCountRef = useRef(0)
  const wheelEventCountRef = useRef(0)
  const [debugInfo, setDebugInfo] = useState({ frames: 0, offset: 0, wheel: 0 })

  // ── Measure cell metrics from DOM ───────────────────────────────────

  const measureCell = useCallback(() => {
    const span = document.createElement('span')
    span.style.cssText = `font-family: 'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    span.textContent = 'W'
    document.body.appendChild(span)
    const rect = span.getBoundingClientRect()
    document.body.removeChild(span)
    const width = rect.width
    const height = Math.ceil(fontSize * 1.4)  // line height multiplier
    cellMetricsRef.current = { width, height }
    setCellMetrics({ width, height })
    return { width, height }
  }, [fontSize])

  // ── Apply grid update ───────────────────────────────────────────────

  const applyGridUpdate = useCallback((update: GridUpdate) => {
    const map = linesRef.current

    if (update.full) {
      map.clear()
    }

    for (const line of update.lines) {
      map.set(line.row, line)
    }

    frameCountRef.current++
    termModeRef.current = update.mode

    setGridState({
      rows: update.rows,
      cols: update.cols,
      cursorCol: update.cursor_col,
      cursorRow: update.cursor_row,
      cursorVisible: update.cursor_visible,
      cursorShape: update.cursor_shape,
      displayOffset: update.display_offset,
      version: frameCountRef.current,
    })

    setDebugInfo({
      frames: frameCountRef.current,
      offset: update.display_offset,
      wheel: wheelEventCountRef.current,
    })
  }, [])

  // ── rAF-batched rendering ──────────────────────────────────────────

  const scheduleRender = useCallback((payload: GridUpdate) => {
    pendingFrameRef.current = payload
    if (rafPendingRef.current) return
    rafPendingRef.current = true
    requestAnimationFrame(() => {
      rafPendingRef.current = false
      const frame = pendingFrameRef.current
      pendingFrameRef.current = null
      if (frame) applyGridUpdate(frame)
    })
  }, [applyGridUpdate])

  // ── Calculate grid dimensions ──────────────────────────────────────

  const calculateDimensions = useCallback(() => {
    const container = containerRef.current
    if (!container) return { cols: 80, rows: 24 }
    const { width, height } = cellMetricsRef.current
    if (width === 0 || height === 0) return { cols: 80, rows: 24 }
    const cols = Math.max(1, Math.floor(container.clientWidth / width))
    const rows = Math.max(1, Math.floor(container.clientHeight / height))
    return { cols, rows }
  }, [])

  // ── Font size changes ──────────────────────────────────────────────

  useEffect(() => {
    measureCell()
    if (!created || !ptyIdRef.current) return
    const { cols, rows } = calculateDimensions()
    if (cols !== lastColsRef.current || rows !== lastRowsRef.current) {
      lastColsRef.current = cols
      lastRowsRef.current = rows
      invoke('terminal_resize', { id: ptyIdRef.current, cols, rows }).catch(() => {})
    }
  }, [fontSize, created, calculateDimensions, measureCell])

  // ── Create terminal + listen for events ────────────────────────────

  useEffect(() => {
    let unlistenGrid: (() => void) | undefined
    let unlistenExit: (() => void) | undefined
    let unlistenDrop: (() => void) | undefined
    let mounted = true

    const setup = async (): Promise<void> => {
      // Measure cell metrics first
      measureCell()

      const exists = await invoke<boolean>('terminal_exists', { id: terminalId })

      if (!exists) {
        const { cols, rows } = calculateDimensions()
        lastColsRef.current = cols
        lastRowsRef.current = rows
        await invoke('terminal_create', {
          id: terminalId, cwd,
          command: command ?? null, args: args ?? null,
          cols, rows,
        })
      }

      // Resize with measured metrics
      const { cols: newCols, rows: newRows } = calculateDimensions()
      if (newCols !== lastColsRef.current || newRows !== lastRowsRef.current) {
        lastColsRef.current = newCols
        lastRowsRef.current = newRows
        await invoke('terminal_resize', { id: terminalId, cols: newCols, rows: newRows }).catch(() => {})
      }

      // Reattach: get current grid state
      if (exists) {
        try {
          const grid = await invoke<GridUpdate>('terminal_get_grid', { id: terminalId })
          if (mounted) applyGridUpdate(grid)
        } catch { /* fallback */ }
      }

      if (!mounted) return
      ptyIdRef.current = terminalId
      setCreated(true)

      invoke('terminal_set_focus', { id: terminalId, focused: true }).catch(() => {})

      // Listen for grid updates (DOM text rendering)
      unlistenGrid = await listen<GridUpdate>(`terminal:grid:${terminalId}`, (event) => {
        scheduleRender(event.payload)
      })

      unlistenExit = await listen<{ exitCode: number }>(`terminal:exit:${terminalId}`, (event) => {
        onExit?.(event.payload.exitCode)
      })

      unlistenDrop = await listen<{ paths: string[] }>('tauri://drag-drop', (event) => {
        if (!ptyIdRef.current || !event.payload.paths?.length) return
        const escaped = event.payload.paths.map(shellEscape).join(' ')
        invoke('terminal_write', { id: ptyIdRef.current, data: escaped + ' ' })
      })
    }

    setup().catch((err) => console.error('[AlacrittyTerminalView] Setup failed:', err))

    return () => {
      mounted = false
      unlistenGrid?.()
      unlistenExit?.()
      unlistenDrop?.()
    }
  }, [terminalId, cwd, command]) // eslint-disable-line react-hooks/exhaustive-deps

  // ── Resize ─────────────────────────────────────────────────────────

  useEffect(() => {
    const container = containerRef.current
    if (!container || !created) return
    let resizeTimer: ReturnType<typeof setTimeout>
    const observer = new ResizeObserver(() => {
      clearTimeout(resizeTimer)
      resizeTimer = setTimeout(() => {
        const { cols, rows } = calculateDimensions()
        if (cols === lastColsRef.current && rows === lastRowsRef.current) return
        lastColsRef.current = cols
        lastRowsRef.current = rows
        if (ptyIdRef.current) {
          invoke('terminal_resize', { id: ptyIdRef.current, cols, rows }).catch(() => {})
        }
      }, 80)
    })
    observer.observe(container)
    return () => { clearTimeout(resizeTimer); observer.disconnect() }
  }, [created, calculateDimensions])

  // ── Focus tracking ─────────────────────────────────────────────────

  const handleFocus = useCallback(() => {
    if (ptyIdRef.current) invoke('terminal_set_focus', { id: ptyIdRef.current, focused: true }).catch(() => {})
  }, [])
  const handleBlur = useCallback(() => {
    if (ptyIdRef.current) invoke('terminal_set_focus', { id: ptyIdRef.current, focused: false }).catch(() => {})
  }, [])

  // ── Keyboard ───────────────────────────────────────────────────────

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (!ptyIdRef.current) return
    const ne = e.nativeEvent

    if (naturalTextEditing) {
      const seq = naturalTextEditingSequence(ne)
      if (seq) {
        e.preventDefault(); e.stopPropagation()
        invoke('terminal_write', { id: ptyIdRef.current, data: seq })
        return
      }
    }

    const data = keyEventToSequence(ne, termModeRef.current)
    if (data) {
      e.preventDefault(); e.stopPropagation()
      invoke('terminal_write', { id: ptyIdRef.current, data })
    }
  }, [naturalTextEditing])

  // ── Paste ──────────────────────────────────────────────────────────

  const handlePaste = useCallback((e: React.ClipboardEvent) => {
    if (!ptyIdRef.current) return
    const text = e.clipboardData.getData('text')
    if (text) { e.preventDefault(); invoke('terminal_write', { id: ptyIdRef.current, data: text }) }
  }, [])

  // ── Scroll — throttled + accumulated ─────────────────────────────────
  // macOS trackpad sends dozens of pixel-delta wheel events per swipe.
  // We accumulate the pixel deltas and send a line-based scroll IPC
  // at most every 50ms to avoid flooding.

  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const handleWheel = useCallback((e: React.WheelEvent) => {
    if (!ptyIdRef.current) return
    wheelEventCountRef.current++

    // Accumulate pixel deltas
    scrollAccumRef.current += e.deltaY

    // If no pending flush, schedule one
    if (!scrollTimerRef.current) {
      scrollTimerRef.current = setTimeout(() => {
        scrollTimerRef.current = null
        const accum = scrollAccumRef.current
        scrollAccumRef.current = 0
        if (accum === 0 || !ptyIdRef.current) return

        // Convert accumulated pixels to lines (1 line ≈ cell height, min 20px)
        const lineHeight = cellMetricsRef.current.height || 20
        const lines = Math.round(accum / lineHeight)
        const delta = -lines
        if (delta !== 0) {
          invoke('terminal_scroll', { id: ptyIdRef.current, delta }).catch(() => {})
        }
      }, 50)
    }
  }, [])

  // ── Cursor blink ───────────────────────────────────────────────────

  useEffect(() => {
    const interval = setInterval(() => setCursorBlinkVisible((v) => !v), 530)
    return () => clearInterval(interval)
  }, [])

  // ── Auto-focus ─────────────────────────────────────────────────────

  useEffect(() => { containerRef.current?.focus() }, [created])

  // ── Render ─────────────────────────────────────────────────────────

  const { width: cellW, height: cellH } = cellMetrics
  // Two cursor modes:
  // 1. App shows cursor (vis:Y, e.g. zsh) — we render our overlay block cursor
  // 2. App hides cursor (vis:N, e.g. Claude Code) — app draws its own via INVERSE cells
  const showCursor = gridState.cursorVisible && gridState.displayOffset === 0

  // Build row elements from the lines map
  const rowElements: React.JSX.Element[] = []
  // Debug: log the cursor row's data (dev only)
  if (import.meta.env.DEV) {
    const cursorRowLine = linesRef.current.get(gridState.cursorRow)
    if (cursorRowLine && cursorRowLine.spans && cursorRowLine.spans.length > 0) {
      const lastSpan = cursorRowLine.spans[cursorRowLine.spans.length - 1]
      const textLen = [...cursorRowLine.text].length
      console.error(`[cursor-row] row=${gridState.cursorRow} textChars=${textLen} textBytes=${cursorRowLine.text.length} spans=${cursorRowLine.spans.length} lastSpan=s${lastSpan.s}..e${lastSpan.e} bg=${lastSpan.bg} fl=${lastSpan.fl}`)
    }
  }
  for (let r = 0; r < gridState.rows; r++) {
    const line = linesRef.current.get(r)
    rowElements.push(
      <div
        key={r}
        style={{
          height: cellH || undefined,
          whiteSpace: 'pre',
          overflow: 'hidden',
          lineHeight: cellH ? `${cellH}px` : undefined,
        }}
      >
        {line ? renderLineSpans(line) : '\u00A0'}
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className="h-full w-full bg-[#0a0a0a] focus:outline-none overflow-hidden"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      onWheel={handleWheel}
      onFocus={handleFocus}
      onBlur={handleBlur}
      style={{ cursor: 'text', position: 'relative' }}
    >
      <div
        ref={viewportRef}
        style={{
          fontFamily: "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
          fontSize: `${fontSize}px`,
          color: colorToCSS(DEFAULT_FG),
          background: colorToCSS(DEFAULT_BG),
          fontVariantLigatures: 'none',
          width: '100%',
          height: '100%',
          overflow: 'hidden',
        }}
      >
        {rowElements}
      </div>

      {/* Cursor overlay — block style, matches iTerm2/Alacritty default */}
      {showCursor && cellW > 0 && cellH > 0 && (
        <div
          style={{
            position: 'absolute',
            left: gridState.cursorCol * cellW,
            top: gridState.cursorRow * cellH,
            width: gridState.cursorShape === 'bar' ? 2.5 : cellW,
            height: gridState.cursorShape === 'underline' ? 3 : cellH,
            marginTop: gridState.cursorShape === 'underline' ? cellH - 3 : 0,
            background: gridState.cursorVisible
              ? 'rgba(240, 240, 240, 0.9)'   // App says cursor visible — bright white
              : 'rgba(240, 240, 240, 0.75)',  // App hid cursor — slightly dimmer but clearly visible
            pointerEvents: 'none',
            zIndex: 10,
          }}
        />
      )}

      {/* Debug overlay — only in dev mode */}
      {import.meta.env.DEV && (
        <div style={{
          position: 'absolute', top: 2, right: 2, padding: '2px 6px',
          background: 'rgba(0,0,0,0.8)', color: '#0f0', fontSize: '10px',
          fontFamily: 'monospace', zIndex: 999, pointerEvents: 'none',
          borderRadius: '3px',
        }}>
          frames:{debugInfo.frames} offset:{debugInfo.offset} wheel:{debugInfo.wheel} cells:{gridState.cols}x{gridState.rows} cursor:{gridState.cursorCol},{gridState.cursorRow} vis:{gridState.cursorVisible?'Y':'N'} cell:{cellW.toFixed(1)}x{cellH}
        </div>
      )}
    </div>
  )
}

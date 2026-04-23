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

import { invoke } from '@tauri-apps/api/core'
import { KesselClient } from './client'
import { useKesselConfig } from './config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { TerminalGrid, type Cell, type GridSnapshot } from './grid'
import { styleToCss, stylesEqual } from './style'
import type { CursorShape, Frame } from './types'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
  MODE_APP_CURSOR,
} from '@/lib/key-mapping'

/** Mirror of the daemon's `GROW_ROWS` constant in
 *  `crates/k2so-core/src/terminal/session_stream_pty.rs`. The daemon
 *  opens every PTY at `max(requested_rows, GROW_ROWS)` and paints the
 *  child TUI into that oversized canvas before SIGWINCH'ing down; the
 *  Kessel client must allocate a matching canvas locally so replay
 *  frames (which were captured at the big size) land where the daemon
 *  painted them. When the daemon emits the `grow_boundary` semantic
 *  marker, the client trims + resizes to the real target, at which
 *  point the overflow rows scroll into scrollback naturally. */
export const KESSEL_GROW_ROWS = 500

export interface SessionStreamViewProps {
  /** SessionId UUID for the daemon's live session. `null` =
   *  optimistic mount: render the grid shell (font, cursor, empty
   *  rows) without connecting the WebSocket. The WS useEffect
   *  short-circuits on null so the pane appears instantly and
   *  transitions to a live view when the spawn resolves without
   *  a second mount cycle. L1.5. */
  sessionId: string | null
  /** Daemon port — from `invoke('daemon_ws_url')`. Ignored while
   *  sessionId is null. */
  port: number
  /** Auth token — from `invoke('daemon_ws_url')`. Ignored while
   *  sessionId is null. */
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

/** Write bytes to the PTY via the `kessel_write` Tauri command.
 *
 *  **Why not fetch?** Browser fetch pays ~3-15ms of overhead per call
 *  (CSP check, URL parse, network layer hop). Per-keystroke that
 *  produces visible lag at fast typing. The Tauri command hits a
 *  persistent reqwest::Client with keep-alive — ~1-3ms per call.
 *
 *  Signature kept compatible with the pre-Tauri version so all
 *  callers (keydown, paste, focus, blur) pass through unchanged.
 *  The `port` + `token` arguments are ignored here; the Tauri
 *  command reads them from its own in-memory cache. */
async function writeToSession(
  _port: number,
  _token: string,
  sessionId: string | null,
  text: string,
): Promise<void> {
  if (sessionId === null) return
  try {
    await invoke<void>('kessel_write', { sessionId, text })
  } catch {
    // Fire-and-forget semantics — don't block the keystroke path on
    // a transient daemon blip. Any persistent failure will show up
    // as missing output and the user will retry.
  }
}

/** Resize the PTY via the `kessel_resize` Tauri command. Same
 *  Tauri-IPC-replacing-fetch pattern as `writeToSession`. */
async function resizeSession(
  _port: number,
  _token: string,
  sessionId: string | null,
  cols: number,
  rows: number,
): Promise<void> {
  if (sessionId === null) return
  try {
    await invoke<void>('kessel_resize', { sessionId, cols, rows })
  } catch {
    /* best-effort; grid already resized locally */
  }
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

/** Memoized single-row renderer. React.memo with a custom comparator:
 *  "skip the render when the row is NOT in the current damage set."
 *  D3. Cuts per-keystroke work in 24-row panes from ~1800 cell
 *  iterations to ~80 (only the typed row re-coalesces spans).
 *
 *  We can't use row-reference equality because TerminalGrid mutates
 *  cells in place — the row array is the same object before and
 *  after a keystroke. The `damaged` flag is the real signal.
 *
 *  CAVEAT for 4.7 C2 (per-cell click targeting): this memo never
 *  changes row identity, and coordinate math in the parent uses
 *  cellMetrics × index so click-to-cell still resolves correctly. */
interface RowRendererProps {
  row: readonly Cell[]
  rowIdx: number
  /** True when this row mutated since the last clearDirty(). False
   *  when the renderer should reuse the prior mounted DOM. */
  damaged: boolean
}

const RowRenderer = React.memo(
  function RowRenderer({ row, rowIdx }: RowRendererProps): React.JSX.Element {
    return <div>{renderRow(row, rowIdx)}</div>
  },
  (prev, next) => {
    // Skip the re-render when the row isn't damaged AND geometry
    // matches. If rowIdx changed (resize shifted indices) force a
    // re-render even when undamaged — the key handles this in most
    // cases but we belt-and-suspenders.
    if (prev.rowIdx !== next.rowIdx) return false
    // Reference inequality on `row` means the visible row at this
    // index is pointing at a DIFFERENT underlying Cell[] — typically
    // what happens when viewportOffset changes (visibleRows[i]
    // swaps between `scrollback[N]` and `grid[K]`). Without this
    // check, scrolling back down from scrollback leaves the bottom
    // of the grid showing stale cells — the memo short-circuits
    // because `damaged` is false for rows that weren't mutated
    // since the last clearDirty(), even though the row they're
    // DISPLAYING just changed identity. `row` stays referentially
    // stable across in-place cell mutations within the same grid
    // row, so the `damaged` check below still covers the keystroke
    // / paint path without false-positive re-renders.
    if (prev.row !== next.row) return false
    if (next.damaged) return false
    return true
  },
)

export function SessionStreamView(props: SessionStreamViewProps): React.JSX.Element {
  const config = useKesselConfig()
  const {
    sessionId,
    port,
    token,
    cols = 80,
    rows = 24,
    // Prop `fontSize` is an explicit override — callers that still
    // pass it win over the config. Default falls through to config
    // so a KesselConfigProvider can change the size app-wide.
    fontSize = config.font.size,
    onReady,
    onError,
    interactive = true,
    autoResize = true,
  } = props

  // TerminalGrid is held in a ref because it's imperative state —
  // Frame events mutate it in place, and we trigger a React rerender
  // via a version counter each animation frame.
  //
  // **Grow-phase initial rows.** The grid is constructed with
  // `KESSEL_GROW_ROWS` (matching the daemon's GROW_ROWS) so replay
  // frames — captured while the daemon's PTY was at 500 rows — land
  // at the same rows the child TUI painted them into. The client's
  // grow_boundary handler below trims + resizes to the real target
  // as soon as the boundary marker arrives, and from then on the
  // grid matches the user's window like any other pane.
  //
  // The grid always starts oversized; rAF rendering is cheap because
  // empty rows coalesce to a single span and render in one DOM node
  // apiece, and the boundary frame lands in the same micro-tick that
  // the replay drain finishes (it's always the last frame in the
  // initial WS burst).
  const gridRef = useRef<TerminalGrid | null>(null)
  const awaitingBoundaryRef = useRef(true)
  if (gridRef.current === null) {
    gridRef.current = new TerminalGrid({
      cols,
      rows: Math.max(rows, KESSEL_GROW_ROWS),
      scrollbackCap: config.scrolling.cap,
      syncUpdateTimeoutMs: config.performance.syncUpdateTimeoutMs,
    })
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
      const grid = gridRef.current!
      // rAF coalescing: if nothing mutated the grid between the
      // previous rAF and now (idle pane, or frames that arrived
      // during a ?2026 sync window and got buffered), skip the
      // snapshot + setState pair entirely. Saves the allocation +
      // React's state-equality check on every idle animation frame.
      if (!grid.isDirty()) return
      const snap = grid.snapshot()
      grid.clearDirty()
      setSnapshot(snap)
    })
  }, [])

  // Scrollback viewport. `viewportOffset` is the number of rows we
  // have scrolled up from the bottom of the combined
  // [scrollback..liveGrid] stream. 0 = pinned to bottom (normal
  // live view). Positive = viewing older content that used to be
  // on-screen.
  //
  // Invariants:
  //   - 0 <= viewportOffset <= scrollback.length
  //   - At offset 0, rendering is bit-identical to snapshot.grid
  //   - Pinning logic: if offset is 0 when new lines scroll into
  //     scrollback, stay at 0 (auto-follow). If offset > 0, bump by
  //     the growth delta so the user's absolute-content view stays
  //     frozen — the content they're reading doesn't slide up out
  //     from under them as new output arrives.
  //   - Any user input (keystroke / paste) snaps back to offset 0.
  //     Matches every real terminal's "type-to-bottom" reflex.
  const [viewportOffset, setViewportOffset] = useState(0)
  const prevScrollbackLenRef = useRef(0)
  useEffect(() => {
    const prev = prevScrollbackLenRef.current
    const curr = snapshot.scrollback.length
    prevScrollbackLenRef.current = curr
    const delta = curr - prev
    if (delta > 0 && viewportOffset > 0) {
      setViewportOffset((o) => Math.min(o + delta, curr))
    }
  }, [snapshot.scrollback.length, viewportOffset])

  // Alt-screen cutover: when the TUI enters ?1049 / ?47, force the
  // viewport back to the bottom and stop honoring wheel scroll into
  // scrollback. Alt screen is an isolated buffer — the only thing
  // to view is what the TUI is painting right now, and "scrolling
  // up through scrollback" from within vim / htop would show the
  // user's shell output from before the TUI started, which is
  // worse than useless.
  useEffect(() => {
    if (snapshot.modes.altScreen && viewportOffset !== 0) {
      setViewportOffset(0)
    }
  }, [snapshot.modes.altScreen, viewportOffset])

  // Synchronized-output silent-TUI watchdog. TerminalGrid has its
  // own internal watchdog that fires when a post-timeout frame
  // arrives, but if the TUI opens ?2026 and then goes completely
  // silent, no frame ever arrives to trigger it. This effect runs
  // a setTimeout that force-drains the pending buffer even with no
  // incoming traffic. Matches alacritty's "sync update should not
  // hang the terminal" safety behavior.
  useEffect(() => {
    if (!snapshot.modes.synchronizedOutput) return
    const timeoutMs = config.performance.syncUpdateTimeoutMs
    if (timeoutMs <= 0) return
    const id = setTimeout(() => {
      gridRef.current?.forceDrain()
      scheduleRender()
    }, timeoutMs)
    return () => clearTimeout(id)
  }, [
    snapshot.modes.synchronizedOutput,
    config.performance.syncUpdateTimeoutMs,
    scheduleRender,
  ])

  // Cursor is always visible (no blink). Rosson's product decision:
  // a stable solid cursor reads more calmly than a pulsing one,
  // especially during rapid output. markActivity still exists to
  // drive the resting-position settle logic below; the blink-phase
  // state was removed.
  const lastActivityRef = useRef(Date.now())
  const markActivity = useCallback(() => {
    lastActivityRef.current = Date.now()
  }, [])

  // Cursor "resting position" tracking. The visible cursor overlay
  // follows `restingCursor`, not `snapshot.cursor`. `snapshot.cursor`
  // faithfully reflects the grid state every rAF — which includes the
  // intermediate positions Claude paints through during a repaint
  // (save → move to bottom border → paint → restore). Rendering the
  // intermediate positions makes the caret visibly jump.
  //
  // Alacritty's frame pacing hides this: a full Claude repaint fits
  // in one 16ms GL frame, so only the final position is ever drawn.
  // Our WS delivery + rAF is coarser — Claude's bytes can arrive
  // across multiple rAF cycles and each one snapshots an intermediate
  // cursor state.
  //
  // Fix: hybrid settle policy based on move magnitude.
  //   - Small moves (≤ 1 row change, ≤ 20 col change) advance the
  //     rendered cursor immediately. Covers typing (col +1), Enter
  //     (row +1, col=0), line wrap, and short cursor repositions.
  //     Keeps fast typing feeling zero-latency.
  //   - Large moves (multi-row jumps away) wait for SETTLE_MS of
  //     quiet before committing. Covers Claude's "save → move to
  //     bottom border → paint → restore" repaint sequence where
  //     the intermediate position is many rows from the rest
  //     position.
  //
  // The quiet threshold is below the 100ms perception-of-latency
  // floor, and above the typical inter-chunk gap we see on Claude's
  // repaint bursts (10-30ms). Pulled from config so users can tune
  // it — higher values trade input latency for more jitter
  // suppression.
  const SETTLE_MS = config.cursor.settleMs
  const [restingCursor, setRestingCursor] = useState<{
    row: number
    col: number
    visible: boolean
    shape: CursorShape | null
  }>(() => ({
    row: 0,
    col: 0,
    visible: true,
    shape: null,
  }))
  useEffect(() => {
    const id = setInterval(() => {
      const s = gridRef.current?.snapshot()
      if (!s) return
      setRestingCursor((prev) => {
        const sameVis = prev.visible === s.cursor.visible
        const sameShape = prev.shape === s.cursor.shape
        const sameRow = prev.row === s.cursor.row
        const sameCol = prev.col === s.cursor.col
        if (sameVis && sameShape && sameRow && sameCol) return prev

        // Visibility transitions commit immediately — the whole
        // point of DECTCEM (CSI ?25 h/l) is "hide cursor NOW during
        // this repaint." Deferring the hide would leak intermediate
        // cursor positions exactly when the TUI asked us not to.
        // Shape transitions (DECSCUSR) likewise — vim's mode
        // indicator is a user-facing correctness signal, not a
        // repaint artifact; users see any lag.
        if (!sameVis || !sameShape) {
          return { ...s.cursor }
        }

        const rowDelta = Math.abs(s.cursor.row - prev.row)
        const colDelta = Math.abs(s.cursor.col - prev.col)
        const isSmallMove = rowDelta <= 1 && colDelta <= 20

        // Special case: cursor snapped to col=0 on the same row it
        // was already on, from ANY non-zero column. This is the
        // `\r`-then-reprint idiom every shell and TUI uses to
        // rewrite the current line (ZLE prompt refresh, `rg`
        // progress updates, Claude Code's input-prompt repaint
        // where the caret sits at col=2 right of `> `, etc.).
        // Without the settle, the caret flickers to col=0 for
        // ~16ms before the reprint catches up.
        //
        // Even prev.col=1 triggers this — Claude's repaint pattern
        // passes through very short prompts too, and the 60ms
        // settle is below the perception-of-latency floor. The
        // only cases that ever feel the lag are Home/Ctrl+A and
        // backspace-at-col=1, both of which are user-initiated
        // so the user is already LOOKING FOR the cursor to move —
        // 60ms is imperceptible.
        const isTransientCR =
          s.cursor.col === 0 &&
          s.cursor.row === prev.row &&
          prev.col >= 1

        if (isSmallMove && !isTransientCR) {
          // Typing / Enter / line wrap — advance immediately.
          return { ...s.cursor }
        }

        // Large move (Claude's bottom-border repaint, \r reprint,
        // etc.). Hold position until activity has been quiet for
        // SETTLE_MS — if the reprint finishes within that window
        // the intermediate col=0 frame never renders.
        const quietMs = Date.now() - lastActivityRef.current
        if (quietMs < SETTLE_MS) return prev
        return { ...s.cursor }
      })
    }, 16)
    return () => clearInterval(id)
  }, [SETTLE_MS])

  // Propagate prop-driven resize into the grid.
  //
  // Suppressed while awaitingBoundaryRef is true — the grid is
  // intentionally oversized during the grow window and must not be
  // shrunk before the daemon's grow_boundary marker arrives. Once
  // boundary-handling fires `grid.resize(target_cols, target_rows)`
  // the ref flips to false and subsequent prop changes apply
  // normally.
  useEffect(() => {
    if (!gridRef.current) return
    if (awaitingBoundaryRef.current) return
    if (gridRef.current.rows !== rows || gridRef.current.cols !== cols) {
      gridRef.current.resize(cols, rows)
      scheduleRender()
    }
  }, [cols, rows, scheduleRender])

  // Open the WS once per (sessionId, port, token) tuple. dispose on
  // unmount or prop change. L1.5: skip entirely while sessionId is
  // null — the pane is still optimistic-mounted waiting for spawn
  // to complete, and there's nothing to subscribe to yet.
  useEffect(() => {
    if (sessionId === null) return
    const wsStart = performance.now()
    let firstFrameAt: number | null = null
    let ackAt: number | null = null
    let altScreenAt: number | null = null
    let tuiReadyAt: number | null = null

    // Safety fallback: if we're still waiting on the daemon's
    // grow_boundary marker 3 s after the ack arrives, assume we're
    // talking to an older daemon that doesn't emit it. Clear
    // awaitingBoundary and force a resize to the current props so
    // the grid doesn't stay stuck at KESSEL_GROW_ROWS forever. No-op
    // when the boundary already fired.
    let boundaryFallback: ReturnType<typeof setTimeout> | null = null
    const client = new KesselClient({
      sessionId,
      port,
      token,
      frameBatchingEnabled: config.performance.frameBatchingEnabled,
    })
    // D4: one applyFrame loop + one scheduleRender per batch. Order
    // preserved (4.7 C4). This cuts the per-frame React setState
    // cascade that Claude's bottom-border repaints used to trigger.
    const off = client.on({
      onFrames: (frames) => {
        const now = performance.now()
        if (firstFrameAt === null) {
          firstFrameAt = now
          // eslint-disable-next-line no-console
          console.info(
            `%c[Kessel] tab-${sessionId.slice(0, 8)} first-frame=${Math.round(firstFrameAt - wsStart)}ms (ack=${
              ackAt !== null ? Math.round(ackAt - wsStart) : '?'
            }ms)`,
            'color:#0ff',
          )
        }
        const grid = gridRef.current!
        for (const frame of frames) {
          // grow_boundary — daemon's marker between "painted into the
          // oversized grow canvas" and "child is now running at the
          // real size." Trim the live grid down to content rows
          // (cursor.row + 1), then resize to the daemon's target so
          // the overflow scrolls into scrollback via grid.resize's
          // standard top-push shrink path. After this point the
          // grid matches the real window and ResizeObserver fires
          // behave normally.
          if (
            awaitingBoundaryRef.current &&
            frame.frame === 'SemanticEvent' &&
            frame.data.kind.type === 'Custom' &&
            frame.data.kind.kind === 'grow_boundary'
          ) {
            const payload = frame.data.kind.payload as {
              target_cols?: number
              target_rows?: number
              grow_rows?: number
              reason?: string
            }
            const tCols = payload.target_cols ?? cols
            const tRows = payload.target_rows ?? rows
            const preCursor = grid.snapshot().cursor
            const preRows = grid.rows
            const preScrollbackLen = grid.snapshot().scrollback.length
            grid.trimRows(preCursor.row + 1)
            grid.resize(tCols, tRows)
            awaitingBoundaryRef.current = false
            if (boundaryFallback !== null) {
              clearTimeout(boundaryFallback)
              boundaryFallback = null
            }
            const postScrollbackLen = grid.snapshot().scrollback.length
            // eslint-disable-next-line no-console
            console.info(
              `%c[Kessel] tab-${sessionId.slice(0, 8)} grow_boundary ` +
                `cursor.row=${preCursor.row} trim=${preRows}→${preCursor.row + 1} ` +
                `resize=${tCols}×${tRows} (reason=${payload.reason}) ` +
                `scrollback:${preScrollbackLen}→${postScrollbackLen}`,
              'color:#ff0;font-weight:bold',
            )
            continue
          }
          // TUI-launch breakdown.
          //
          // Original version fired tui-ready only after an
          // alt-screen enter — useful for vim/htop but WRONG for
          // Claude, which doesn't use alt-screen. Claude paints
          // inline in the scrollback buffer. We need a TUI-agnostic
          // signal for "the TUI has started painting its UI."
          //
          // New heuristic: tui-alt-screen still fires on alt-screen
          // for TUIs that use it. tui-ready fires when we see ANY
          // ModeChange frame that signals interactive intent —
          // specifically `bracketed_paste: on` or `focus_reporting:
          // on`, which Claude + most modern TUIs emit right after
          // their cold-start is done and before they paint. On
          // alt-screen TUIs, tui-ready fires at the same time as
          // tui-alt-screen. Works uniformly for Claude (no alt-
          // screen) and vim (alt-screen + bracketed paste).
          if (
            altScreenAt === null &&
            frame.frame === 'ModeChange' &&
            frame.data.mode === 'alt_screen' &&
            frame.data.on === true
          ) {
            altScreenAt = now
            // eslint-disable-next-line no-console
            console.info(
              `%c[Kessel] tab-${sessionId.slice(0, 8)} tui-alt-screen=${Math.round(altScreenAt - wsStart)}ms`,
              'color:#0ff',
            )
          }
          if (
            tuiReadyAt === null &&
            frame.frame === 'ModeChange' &&
            frame.data.on === true &&
            (frame.data.mode === 'alt_screen' ||
              frame.data.mode === 'bracketed_paste' ||
              frame.data.mode === 'focus_reporting')
          ) {
            tuiReadyAt = now
            const totalToTui = Math.round(tuiReadyAt - wsStart)
            // eslint-disable-next-line no-console
            console.info(
              `%c[Kessel] tab-${sessionId.slice(0, 8)} tui-ready=${totalToTui}ms (${frame.data.mode} ON → TUI is interactive)`,
              'color:#0ff;font-weight:bold',
            )
          }
          grid.applyFrame(frame)
        }
        markActivity()
        scheduleRender()
      },
      onAck: (ack) => {
        ackAt = performance.now()
        // eslint-disable-next-line no-console
        console.info(
          `%c[Kessel] tab-${sessionId.slice(0, 8)} ws-ack=${Math.round(ackAt - wsStart)}ms replay=${ack.replayCount}`,
          'color:#0ff',
        )
        onReady?.(ack.replayCount)
        // Arm the fallback: 3 s after ack, if we still haven't seen
        // a grow_boundary frame, drop out of the grow-phase gate so
        // the grid doesn't stay stuck at KESSEL_GROW_ROWS. Covers
        // the "old daemon" and "daemon crashed mid-grow" cases.
        if (boundaryFallback === null) {
          boundaryFallback = setTimeout(() => {
            if (!awaitingBoundaryRef.current) return
            const grid = gridRef.current
            if (!grid) return
            const preRows = grid.rows
            const preCursor = grid.snapshot().cursor
            // Prefer measuring the live container over the props
            // `cols`/`rows` defaults (80x24) — the fallback runs
            // when we're talking to an older daemon that never
            // sends the boundary, and in that case the daemon has
            // already shrunk the PTY to whatever the spawn request
            // asked for. We want the client-side grid to match the
            // REAL window so ResizeObserver doesn't have to
            // immediately resize again.
            const el = containerRef.current
            const cm = cellMetrics
            let targetCols = cols
            let targetRows = rows
            if (el && cm.width > 0 && cm.height > 0) {
              const rect = el.getBoundingClientRect()
              const w = Math.max(0, rect.width - 8)
              const h = Math.max(0, rect.height - 8)
              const c = Math.floor(w / cm.width)
              const r = Math.floor(h / cm.height)
              if (c >= 10 && r >= 3) {
                targetCols = c
                targetRows = r
              }
            }
            grid.trimRows(preCursor.row + 1)
            grid.resize(targetCols, targetRows)
            awaitingBoundaryRef.current = false
            scheduleRender()
            // eslint-disable-next-line no-console
            console.warn(
              `%c[Kessel] tab-${sessionId.slice(0, 8)} grow_boundary fallback ` +
                `(no marker after 3s) trim=${preRows}→${preCursor.row + 1} ` +
                `resize=${targetCols}×${targetRows} ` +
                `(measured; daemon is old?)`,
              'color:#fa0;font-weight:bold',
            )
          }, 3000)
        }
      },
      onError: (err) => onError?.(err.message),
    })
    client.connect()
    return () => {
      off()
      client.dispose()
      if (boundaryFallback !== null) {
        clearTimeout(boundaryFallback)
        boundaryFallback = null
      }
    }
  }, [
    sessionId,
    port,
    token,
    onReady,
    onError,
    scheduleRender,
    markActivity,
    config.performance.frameBatchingEnabled,
  ])

  // Cell metrics for cursor positioning. Measured once per fontSize
  // change by writing a hidden span and reading its box. Simple and
  // accurate; matches AlacrittyTerminalView's approach.
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })

  // Focus tracking drives solid-vs-hollow cursor rendering. Initial
  // value reflects whether the tab owns focus at mount; focus/blur
  // listeners (attached further below, after containerRef is
  // declared) keep it in sync. Decoupled from the D7 focus-reporting
  // effect — that one only writes CSI I/O when the TUI opted in,
  // whereas the cursor UX must always reflect focus.
  const [isFocused, setIsFocused] = useState<boolean>(() => {
    if (typeof document === 'undefined') return false
    return document.hasFocus()
  })
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

  // Keyboard input path (I6). Keydown → key-mapping encoder →
  // daemon's /cli/terminal/write. Focus is grabbed on mount when
  // interactive=true so the pane is keyboard-reachable without the
  // user having to click first.
  const containerRef = useRef<HTMLDivElement>(null)

  // Focus-tracking listener for the cursor UX (paired with the
  // `isFocused` state declared above). Runs unconditionally —
  // non-interactive panes still need the hollow-cursor affordance
  // when the user clicks away.
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const onFocus = (): void => setIsFocused(true)
    const onBlur = (): void => setIsFocused(false)
    el.addEventListener('focus', onFocus)
    el.addEventListener('blur', onBlur)
    return () => {
      el.removeEventListener('focus', onFocus)
      el.removeEventListener('blur', onBlur)
    }
  }, [])

  // Auto-focus when the tab becomes visible. Retained-view model:
  // every pane stays mounted across tab switches (display:none on
  // inactive ones), so the keydown effect's mount-time el.focus()
  // only fires once per pane lifetime. Without this, clicking the
  // tab bar surfaces the pane but leaves focus on the tab button —
  // the user has to click inside the terminal before typing
  // registers. Match the behavior of every native tabbed terminal:
  // on tab activation, the terminal owns focus.
  //
  // Gated on `interactive` — a read-only Kessel preview (HarnessLab
  // embed, future read-only viewers) shouldn't steal focus.
  const isTabVisible = useIsTabVisible()
  useEffect(() => {
    if (!interactive) return
    if (!isTabVisible) return
    const el = containerRef.current
    if (!el) return
    // Defer one frame so the pane's `display: block` transition has
    // painted before focus runs. Calling focus() on a still-hidden
    // element is a no-op in some browsers.
    const raf = requestAnimationFrame(() => {
      containerRef.current?.focus()
    })
    return () => cancelAnimationFrame(raf)
  }, [interactive, isTabVisible])

  useEffect(() => {
    if (!interactive) return
    const el = containerRef.current
    if (!el) return
    const handler = (e: KeyboardEvent) => {
      // macOS natural-text-editing shortcuts first — Cmd+Arrow,
      // Option+Arrow, Option+Backspace, Cmd+Backspace (→ Ctrl+U,
      // kill-line-to-beginning), Cmd+Delete (→ Ctrl+K). These bind
      // higher-level semantics onto keys that `keyEventToSequence`
      // would otherwise return null for (since meta chords default
      // to "let the browser handle it"). Parity with how the
      // legacy AlacrittyTerminalView feels.
      const natural = naturalTextEditingSequence(e)
      if (natural !== null) {
        e.preventDefault()
        markActivity()
        setViewportOffset(0)
        writeToSession(port, token, sessionId, natural).catch((err) => {
          // eslint-disable-next-line no-console
          console.warn('[kessel] write failed:', err)
        })
        return
      }
      // D5 — read the live application-cursor flag off the grid
      // so zsh / vim get SS3-format arrow keys when they flip
      // DECSET ?1. Reading gridRef rather than snapshot.modes
      // because keystrokes can arrive between rAF ticks and the
      // React state snapshot might lag the real mode state by one
      // frame.
      const modeFlags = gridRef.current?.snapshot().modes.appCursor
        ? MODE_APP_CURSOR
        : 0
      const seq = keyEventToSequence(e, modeFlags)
      if (seq === null) return
      e.preventDefault()
      // Mark activity so the resting-cursor settle effect knows a
      // burst is in progress and defers large-move commits.
      markActivity()
      // Snap back to bottom whenever the user types — matches real
      // terminal behavior. If we're already at bottom this is a
      // no-op.
      setViewportOffset(0)
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
      markActivity()
      setViewportOffset(0)
      // Bracketed-paste wrap: if the TUI asked for it via ?2004 h,
      // frame the paste between ESC[200~ and ESC[201~. This is what
      // lets Claude / readline / etc. distinguish a paste burst
      // from real keystrokes (otherwise a multi-line paste into
      // Claude's prompt auto-submits at the first newline).
      //
      // The mode flag is maintained by TerminalGrid.handleModeChange
      // as Frames arrive; we read the current snapshot here rather
      // than depending on snapshot React state, since paste can
      // arrive between rAF cycles.
      const modes = gridRef.current?.snapshot().modes
      const payload = modes?.bracketedPaste
        ? `\x1b[200~${text}\x1b[201~`
        : text
      writeToSession(port, token, sessionId, payload).catch((err) => {
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
  }, [interactive, port, token, sessionId, markActivity])

  // D14 — bell visual flash. When snapshot.bellCount increments
  // (TUI emitted BEL), add a CSS class for config.bell.durationMs
  // that paints a translucent overlay, then clear it. Multiple
  // bells inside the flash window extend the duration — the timer
  // is cleared + reset on every increment. Audio is delegated to
  // the system bell when config.bell.mode is 'audio' or 'both'.
  const [bellFlashing, setBellFlashing] = useState(false)
  useEffect(() => {
    if (snapshot.bellCount === 0) return
    if (config.bell.mode === 'off') return
    if (config.bell.mode === 'visual' || config.bell.mode === 'both') {
      setBellFlashing(true)
      const id = setTimeout(() => setBellFlashing(false), config.bell.durationMs)
      return () => clearTimeout(id)
    }
    return undefined
  }, [snapshot.bellCount, config.bell.mode, config.bell.durationMs])

  // Audio cue — separate effect so it can fire independently of
  // the visual flash timing. Uses a short programmatic AudioContext
  // beep to avoid bundling a sound file and to respect the OS
  // output-device routing.
  useEffect(() => {
    if (snapshot.bellCount === 0) return
    if (config.bell.mode !== 'audio' && config.bell.mode !== 'both') return
    try {
      const AudioCtx = (window as unknown as {
        AudioContext?: typeof AudioContext
        webkitAudioContext?: typeof AudioContext
      })
      const AC = AudioCtx.AudioContext ?? AudioCtx.webkitAudioContext
      if (!AC) return
      const ctx = new AC()
      const osc = ctx.createOscillator()
      const gain = ctx.createGain()
      osc.frequency.value = 880
      gain.gain.setValueAtTime(0.0001, ctx.currentTime)
      gain.gain.exponentialRampToValueAtTime(0.05, ctx.currentTime + 0.01)
      gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + 0.12)
      osc.connect(gain).connect(ctx.destination)
      osc.start()
      osc.stop(ctx.currentTime + 0.12)
      setTimeout(() => ctx.close().catch(() => {}), 200)
    } catch {
      // Environments without AudioContext (tests, headless) — the
      // visual flash covers the case. No throw.
    }
  }, [snapshot.bellCount, config.bell.mode])

  // D7 — focus reporting. When the TUI has enabled DECSET ?1004,
  // write CSI I on focus and CSI O on blur so neovim / tmux / etc.
  // can dim their UI while unfocused. The listener is attached
  // unconditionally (both focus and blur always fire) but the write
  // is gated on the mode flag — dispatching events to a TUI that
  // didn't ask for them would paint junk bytes into its prompt.
  useEffect(() => {
    if (!interactive) return
    const el = containerRef.current
    if (!el) return
    const onFocus = (): void => {
      if (!gridRef.current?.snapshot().modes.focusReporting) return
      writeToSession(port, token, sessionId, '\x1b[I').catch(() => {})
    }
    const onBlur = (): void => {
      if (!gridRef.current?.snapshot().modes.focusReporting) return
      writeToSession(port, token, sessionId, '\x1b[O').catch(() => {})
    }
    el.addEventListener('focus', onFocus)
    el.addEventListener('blur', onBlur)
    return () => {
      el.removeEventListener('focus', onFocus)
      el.removeEventListener('blur', onBlur)
    }
  }, [interactive, port, token, sessionId])

  // Mouse wheel → scrollback navigation.
  //
  // macOS trackpads fire 30-60 pixel-delta wheel events per two-
  // finger swipe. Treating each event as a discrete "tick" would
  // scroll hundreds of lines per swipe. The fix — ported from
  // AlacrittyTerminalView — is:
  //   1. Accumulate pixel deltas (`e.deltaY`) across events.
  //   2. Flush every 50ms via setTimeout.
  //   3. Convert accumulated pixels → lines at 1 line per cell
  //      height. A 100px swipe on a 20px cell = 5 lines, matching
  //      physical-distance scrolling in every native macOS app.
  //
  // DOM_DELTA_LINE (rare — classic mouse wheels on some drivers)
  // arrives as a small integer line count rather than pixels. We
  // scale those up by cellHeight so a single accumulator variable
  // can carry both kinds.
  //
  // `config.scrolling.multiplier` is a user-tunable sensitivity
  // factor: 1.0 matches Alacritty 1:1 (the default). Bump it up
  // for faster scrolling, down for slower.
  //
  // deltaY < 0 → older content (increase viewportOffset).
  // deltaY > 0 → newer content (decrease viewportOffset).
  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const FLUSH_MS = 50
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0) return
      const snap = gridRef.current?.snapshot()
      // While the TUI owns the alt screen buffer (vim / htop etc.),
      // scrollback doesn't apply — let the event pass through in
      // case the TUI has its own mouse-wheel handling (Phase 5
      // mouse reporting will forward it instead).
      if (snap?.modes.altScreen) return
      e.preventDefault()

      const cellH = cellMetrics.height || 20
      // Normalize deltaY to pixels regardless of deltaMode so the
      // accumulator math below is one-size-fits-all.
      const pixelDelta =
        e.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? e.deltaY * cellH
          : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? e.deltaY * cellH * (snap?.rows ?? 24)
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
          const offsetDelta = -lines
          const maxOffset =
            gridRef.current?.snapshot().scrollback.length ?? 0
          setViewportOffset((o) => {
            const next = o + offsetDelta
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
  }, [config.scrolling.multiplier, cellMetrics.height])

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
        // Tab switches use `display:none` to hide inactive panes
        // (see PaneGroupView.tsx). A hidden container measures as
        // 0×0 via ResizeObserver. If we treated that as a "real"
        // resize, we'd:
        //   1. Floor to MIN_COLS/MIN_ROWS (10×3 — the old behavior).
        //   2. Shrink the grid, truncating every row to 10 cells
        //      and pushing bottom-overflow rows to scrollback at
        //      that narrow width.
        //   3. Send a SIGWINCH to Claude Code / vim / etc., which
        //      repaints at 10×3 — destroying the TUI's rendering
        //      work.
        //   4. When the tab returns and we resize back to real
        //      dimensions, the damage is already done: scrollback
        //      shows narrow-wrapped junk (one word per line with
        //      trailing blanks from the 0.34.1 pad-on-resize fix),
        //      and Claude has to redraw from scratch at full
        //      width — producing the visible message-doubling
        //      users reported after 0.34.1 shipped.
        //
        // Fix: skip the resize entirely if the container has no
        // layout. When the tab becomes visible again, ResizeObserver
        // will fire with real dimensions and we'll either match
        // the current grid size (no-op via the lastCols check
        // below) or resize to the correct new value if the window
        // was actually resized while hidden.
        if (rect.width === 0 || rect.height === 0) return
        // Subtract the 4px padding on each side.
        const availW = Math.max(0, rect.width - 8)
        const availH = Math.max(0, rect.height - 8)
        // Belt-and-braces sanity floor — a real terminal container
        // is hundreds of px wide, but a layout glitch could produce
        // a handful of pixels. Treat anything that'd compute to
        // fewer than MIN_COLS legitimate columns as "layout not
        // settled yet" and skip. This is NOT the clamp the old
        // code did; it's an early return, leaving the grid at its
        // previous (known-good) size until a real measurement
        // arrives.
        const MIN_COLS = 10
        const MIN_ROWS = 3
        const rawCols = Math.floor(availW / cellMetrics.width)
        const rawRows = Math.floor(availH / cellMetrics.height)
        if (rawCols < MIN_COLS || rawRows < MIN_ROWS) return
        const newCols = rawCols
        const newRows = rawRows
        if (newCols === lastCols && newRows === lastRows) return
        lastCols = newCols
        lastRows = newRows
        // Grow-phase gate: if we're still waiting on the daemon's
        // grow_boundary marker, do NOT shrink the oversized grid
        // from under the replay frames. The boundary handler picks
        // the final dims from the payload; after it fires this
        // observer resumes normal operation.
        if (awaitingBoundaryRef.current) return
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
      fontFamily: config.font.family,
      fontSize: `${fontSize}px`,
      lineHeight: `${Math.ceil(fontSize * config.font.lineHeightMultiplier)}px`,
      color: `rgb(${(config.colors.foreground >> 16) & 0xff},${(config.colors.foreground >> 8) & 0xff},${config.colors.foreground & 0xff})`,
      backgroundColor: `rgb(${(config.colors.background >> 16) & 0xff},${(config.colors.background >> 8) & 0xff},${config.colors.background & 0xff})`,
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

  // Compose the visible rows from the combined [scrollback..liveGrid]
  // stream based on viewportOffset. When offset is 0, this is exactly
  // snapshot.grid (identity — no allocation). When offset > 0, the
  // window slides up into scrollback. Missing rows (when scrollback
  // is shorter than the window needs) render as blank — matches
  // alacritty's behavior when you scroll past the oldest line.
  const visibleRows = useMemo<readonly (readonly Cell[])[]>(() => {
    if (viewportOffset === 0) return snapshot.grid
    const { scrollback, grid, rows } = snapshot
    const totalLen = scrollback.length + grid.length
    const windowEnd = totalLen - viewportOffset
    const windowStart = windowEnd - rows
    const out: (readonly Cell[])[] = []
    for (let i = 0; i < rows; i++) {
      const abs = windowStart + i
      if (abs < 0) {
        out.push([])
      } else if (abs < scrollback.length) {
        out.push(scrollback[abs])
      } else {
        out.push(grid[abs - scrollback.length])
      }
    }
    return out
  }, [viewportOffset, snapshot])

  // Track the last cursor position the user saw while the pane
  // was focused. When focus is lost, TUIs with focus-reporting
  // (DECSET ?1004 — Claude Code, neovim, tmux) re-render without
  // the active-input highlight and often "park" the hardware
  // cursor at the bottom-left corner of their drawn area, out of
  // the way. If we naively drove the cursor from the live
  // `restingCursor`, it would visibly jump to the parked row the
  // moment focus leaves — typically below the bypass-permissions
  // footer in Claude's case. Instead, freeze the cursor at the
  // last focused position whenever the pane is not in a
  // steady-state focused mode.
  //
  // FOCUS GAIN has the same race in reverse: the moment the user
  // clicks into the pane we send CSI I (focus-in) to the TUI, but
  // the TUI takes 50-200ms to repaint itself with the cursor back
  // at the input line. During that window, `restingCursor` is
  // still at the parked position. If we flip straight to live the
  // instant `isFocused` becomes true, the user sees a 1-2 frame
  // flash of the solid cursor at the parked position before Claude
  // repaints. Debounce by 200ms — within that window we keep
  // showing the previously-captured position (now solid-filled
  // instead of hollow), so the transition is "hollow → solid in
  // place" rather than "hollow → solid elsewhere → solid in place".
  const lastFocusedCursorRef = useRef(restingCursor)
  const [liveCursorEnabled, setLiveCursorEnabled] = useState(false)
  useEffect(() => {
    if (!isFocused) {
      setLiveCursorEnabled(false)
      return
    }
    const id = setTimeout(() => setLiveCursorEnabled(true), 200)
    return () => clearTimeout(id)
  }, [isFocused])
  useEffect(() => {
    // Only capture the live cursor into the ref once we're
    // confidently in steady-state focused mode. Otherwise the
    // ref would inherit the parked position that Claude painted
    // during the blur → focus round-trip.
    if (isFocused && liveCursorEnabled) {
      lastFocusedCursorRef.current = restingCursor
    }
  }, [isFocused, liveCursorEnabled, restingCursor])

  const cursorStyle = useMemo<React.CSSProperties>(() => {
    // Drive from `restingCursor` (settled position) instead of
    // `snapshot.cursor` (which tracks every intermediate move). See
    // the resting-cursor effect above for rationale.
    //
    // Intentionally ignore `restingCursor.visible` (DECTCEM hide).
    // TUI apps like Claude Code hide the cursor so they can paint
    // their own, but our DOM pane has no way to render that glyph —
    // the user ends up "losing" the cursor entirely. Per the
    // product directive: always show the pane's cursor, solid when
    // the tab is focused, hollow outline when it isn't. Matches the
    // native macOS text-input caret convention.
    //
    // Still hide while the viewport is scrolled up — the cursor is
    // a live-grid coordinate and would render at a row that's
    // showing historical scrollback content.
    if (viewportOffset > 0) return { display: 'none' }
    if (!cellMetrics.width) return { display: 'none' }

    // When unfocused (or within the 200ms post-focus-gain settle
    // window), render the cursor at its last-focused position. Only
    // switch to the live cursor once we're confident Claude / the
    // TUI has finished repainting in response to the CSI I focus-in
    // notification. See lastFocusedCursorRef + liveCursorEnabled
    // above for the full rationale.
    const effective =
      isFocused && liveCursorEnabled
        ? restingCursor
        : lastFocusedCursorRef.current

    // D13 — shape from TUI's DECSCUSR (effective.shape) or the
    // user's configured fallback. Blinking variants only animate
    // when focused; an unfocused pane's cursor is a static outline.
    const shape: CursorShape = effective.shape ?? config.cursor.defaultShape
    const base: React.CSSProperties = {
      position: 'absolute',
      left: `${4 + cellMetrics.width * effective.col}px`,
      top: `${4 + cellMetrics.height * effective.row}px`,
      pointerEvents: 'none',
      boxSizing: 'border-box',
    }
    const blinkMs = config.cursor.blinkIntervalMs
    const animation =
      isFocused && shape.startsWith('blinking_')
        ? `kessel-cursor-blink ${blinkMs * 2}ms steps(2, end) infinite`
        : undefined
    const barWidth = Math.max(1, Math.round(cellMetrics.width * config.cursor.thickness))
    const underscoreHeight = Math.max(1, Math.round(cellMetrics.height * config.cursor.thickness))
    const caretColor = 'rgb(224, 224, 224)'
    const fill = isFocused ? caretColor : 'transparent'
    const outline = isFocused ? undefined : `inset 0 0 0 1px ${caretColor}`

    switch (shape) {
      case 'steady_block':
      case 'blinking_block':
        return {
          ...base,
          width: `${cellMetrics.width}px`,
          height: `${cellMetrics.height}px`,
          backgroundColor: fill,
          boxShadow: outline,
          animation,
        }
      case 'steady_bar':
      case 'blinking_bar':
        return {
          ...base,
          width: `${barWidth}px`,
          height: `${cellMetrics.height}px`,
          backgroundColor: fill,
          boxShadow: outline,
          animation,
        }
      case 'steady_underscore':
      case 'blinking_underscore':
        return {
          ...base,
          width: `${cellMetrics.width}px`,
          height: `${underscoreHeight}px`,
          // Nudge to the bottom of the cell.
          top: `${4 + cellMetrics.height * effective.row + cellMetrics.height - underscoreHeight}px`,
          backgroundColor: fill,
          boxShadow: outline,
          animation,
        }
    }
  }, [
    restingCursor,
    cellMetrics.width,
    cellMetrics.height,
    viewportOffset,
    isFocused,
    liveCursorEnabled,
    config.cursor.defaultShape,
    config.cursor.blinkIntervalMs,
    config.cursor.thickness,
  ])

  // D3 per-row damage lookup. Used by RowRenderer's memo predicate.
  //
  // When viewportOffset > 0 we conservatively treat every row as
  // damaged because the viewport-to-grid mapping shifted. The
  // common case is viewportOffset === 0 where we ride the set.
  //
  // If the whole scrollback viewport is live-grid (altScreen + no
  // scrollback), `abs < scrollback.length` is never true so we
  // translate directly.
  const damageSet = useMemo(
    () => new Set(snapshot.damagedRows),
    [snapshot.damagedRows],
  )
  const isRowDamaged = useCallback(
    (visibleIdx: number): boolean => {
      if (viewportOffset > 0) return true
      return damageSet.has(visibleIdx)
    },
    [damageSet, viewportOffset],
  )

  return (
    <div
      ref={containerRef}
      className="kessel-session-stream-view"
      data-session-id={sessionId}
      tabIndex={interactive ? 0 : -1}
      style={{ ...containerStyle, outline: 'none' }}
    >
      {/* D13 — keyframes for the DECSCUSR blinking variants. Emitted
       *  here so the stylesheet is co-located with the pane; browsers
       *  dedupe identical rules across multiple panes. Opacity-only
       *  animation is GPU-cheap and pauses automatically when the
       *  tab is backgrounded. */}
      <style>{`@keyframes kessel-cursor-blink { 0%,50% { opacity: 1 } 51%,100% { opacity: 0 } }`}</style>
      {visibleRows.map((row, rowIdx) => (
        <RowRenderer
          key={`row-${rowIdx}`}
          row={row}
          rowIdx={rowIdx}
          damaged={isRowDamaged(rowIdx)}
        />
      ))}
      <div aria-hidden="true" style={cursorStyle} />
      {/* D14 — bell visual flash overlay. Opacity fades via transition
       *  to avoid a jarring on/off switch. Pointer-events none so it
       *  doesn't intercept clicks. */}
      <div
        aria-hidden="true"
        style={{
          position: 'absolute',
          inset: 0,
          backgroundColor: `rgba(${(config.bell.color >> 16) & 0xff},${(config.bell.color >> 8) & 0xff},${config.bell.color & 0xff},0.12)`,
          opacity: bellFlashing ? 1 : 0,
          transition: `opacity ${Math.round(config.bell.durationMs / 2)}ms ease-out`,
          pointerEvents: 'none',
        }}
      />
      {/* Dev-mode renderer badge. Mirrors AlacrittyTerminalView's
       *  overlay so we can tell at a glance which renderer a pane
       *  is running — the two look visually near-identical, and
       *  the renderer-setting toggle stamps at tab-creation time
       *  (not hot-swap), so "did my setting actually take?" is an
       *  easy question to answer from this line.
       *
       *  Colour (cyan) deliberately differs from Alacritty's green
       *  so a side-by-side screenshot makes the split obvious. */}
      {import.meta.env.DEV && (
        <div
          style={{
            position: 'absolute',
            top: 2,
            right: 2,
            padding: '2px 6px',
            background: 'rgba(0,0,0,0.8)',
            color: '#0ff',
            fontSize: '10px',
            fontFamily: 'monospace',
            zIndex: 999,
            pointerEvents: 'none',
            borderRadius: '3px',
          }}
        >
          <strong style={{ color: '#fff' }}>Kessel</strong> · cells:
          {snapshot.cols}x{snapshot.rows} cursor:
          {snapshot.cursor.col},{snapshot.cursor.row} vis:
          {snapshot.cursor.visible ? 'Y' : 'N'} shape:
          {snapshot.cursor.shape ?? config.cursor.defaultShape} off:
          {viewportOffset} scr:{snapshot.scrollback.length} bells:
          {snapshot.bellCount}
          {snapshot.modes.altScreen && ' ALT'}
          {snapshot.modes.synchronizedOutput && ' SYNC'}
          {snapshot.modes.appCursor && ' APPCUR'}
          {snapshot.modes.bracketedPaste && ' BP'}
          {!snapshot.modes.autowrap && ' NOWRAP'}
        </div>
      )}
    </div>
  )
}

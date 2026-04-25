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
import { invoke } from '@tauri-apps/api/core'

import { useKesselConfig } from '../kessel/config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
} from '@/lib/key-mapping'
import { getDaemonWs, invalidateDaemonWs } from '../kessel/daemon-ws'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useTabsStore } from '@/stores/tabs'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { detectWorkingSignal } from '@/lib/agent-signals'
import {
  detectLinks,
  type DetectedLink,
} from '@/components/Terminal/terminalLinkDetector'
import {
  bracketPaste,
  isImagePath,
  quotePathForImageDrop,
} from '@/lib/file-drag'

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

function renderRowRuns(row: CellRun[], absRow: number): React.ReactNode {
  if (row.length === 0) return '\u00a0'
  const spans: React.ReactNode[] = []
  for (let i = 0; i < row.length; i++) {
    const run = row[i]
    spans.push(
      <span key={`a${absRow}s${i}`} style={runStyle(run)}>
        {run.text || '\u00a0'}
      </span>,
    )
  }
  return spans
}

/** Join all run text in a row into a single plain string. Used
 *  for link detection (which operates on raw text). */
function rowToText(row: CellRun[]): string {
  let out = ''
  for (const run of row) out += run.text
  return out
}

/** Shell-escape a path for safe paste into a terminal input line.
 *  Mirrors the helper in AlacrittyTerminalView.tsx — duplicated
 *  rather than imported to keep v2 decoupled from v1. */
function shellEscape(path: string): string {
  return path.replace(/[ '"\\()&|;<>$`!#*?[\]{}~]/g, '\\$&')
}

/** Images/PDFs skip backslash-escape so Claude Code's
 *  `[Image #N]` detection (which fs.exists()s the literal string)
 *  can resolve them. */
function formatPathForTerminal(path: string): string {
  return isImagePath(path) ? quotePathForImageDrop(path) : shellEscape(path)
}

/** Build terminal payload for a dropped/pasted set of paths.
 *  Wraps in bracketed paste if any path is an image, so Claude's
 *  paste-event handler fires. */
function buildDropPayload(paths: string[]): string {
  const formatted = paths.map(formatPathForTerminal).join(' ')
  const trailing = formatted + ' '
  return paths.some(isImagePath) ? bracketPaste(trailing) : trailing
}

/** Whether a snapshot's visible grid contains any non-blank cell.
 *  Used by the [v2-perf] instrumentation to detect when the child
 *  process actually paints something (e.g. shell prompt). Empty
 *  initial snapshots are expected on cold spawn — the daemon's Term
 *  has no content until the child writes its first bytes. */
function isGridEmpty(snap: TermGridSnapshot): boolean {
  for (const row of snap.grid) {
    for (const run of row) {
      if (run.text && run.text.trim().length > 0) return false
    }
  }
  return true
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
  /** Parent tab id — used to route file-link clicks to the right
   *  sibling pane when the user's "open links in split pane"
   *  preference is on. */
  tabId?: string
  /** This pane's pane-group id, for the same split-pane routing. */
  paneGroupId?: string
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
  const {
    terminalId,
    tabId,
    paneGroupId,
    cwd,
    command,
    args,
    spawnedAt,
  } = props

  // Live-subscribe to the terminal settings store so Cmd+Shift+=
  // / Cmd+Shift+- menu events (wired via listen('terminal:zoom-*')
  // in terminal-settings.ts) update this component's font size
  // immediately. Prop takes precedence for tests / ad-hoc consumers
  // that want to override.
  const storeFontSize = useTerminalSettingsStore((s) => s.fontSize)
  const fontSize = props.fontSize ?? storeFontSize
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)

  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const [snapshot, setSnapshot] = useState<TermGridSnapshot | null>(null)
  const [viewportOffset, setViewportOffset] = useState(0)
  const [isFocused, setIsFocused] = useState<boolean>(() =>
    typeof document !== 'undefined' ? document.hasFocus() : false,
  )

  const containerRef = useRef<HTMLDivElement>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const isTabVisible = useIsTabVisible()

  // ── A7.5 perf instrumentation (DEV-only) ─────────────────────
  // mountT0 is captured once via lazy useRef init so re-renders
  // don't reset it. Stage timings accumulate into stageMsRef so
  // SUMMARY can break down totals at first_render / tui_first_paint.
  const mountT0Ref = useRef<number | null>(null)
  if (mountT0Ref.current === null) mountT0Ref.current = performance.now()
  const stageMsRef = useRef<Record<string, number>>({})
  const firstSnapshotEmptyRef = useRef<boolean>(true)
  const firstSnapshotSeenRef = useRef<boolean>(false)
  const firstSnapshotReusedRef = useRef<boolean | null>(null)
  const firstRenderFiredRef = useRef<boolean>(false)
  const tuiFirstPaintFiredRef = useRef<boolean>(false)

  const perfLog = useCallback(
    (stage: string, extra?: Record<string, unknown>) => {
      if (!import.meta.env.DEV) return
      const t = performance.now() - (mountT0Ref.current ?? performance.now())
      stageMsRef.current[stage] = t
      let line = `[v2-perf] t=${t.toFixed(0)}ms stage=${stage}`
      if (extra) {
        for (const [k, v] of Object.entries(extra)) {
          line += ` ${k}=${v}`
        }
      }
      // eslint-disable-next-line no-console
      console.info(line)
    },
    [],
  )

  // Link detection state. Set on hover over a URL / file path
  // that `detectLinks` recognizes in the row the mouse is over.
  // Non-null → cursor becomes pointer and click opens the link.
  const [hoveredLink, setHoveredLink] = useState<{
    row: number
    link: DetectedLink
  } | null>(null)
  const cmdHeldRef = useRef(false)
  const mouseDownLinkRef = useRef<DetectedLink | null>(null)
  const lastDetectPosRef = useRef({ x: 0, y: 0 })
  const lastDetectTimeRef = useRef(0)

  // ── Activity detection ────────────────────────────────────────
  // Mirrors AlacrittyTerminalView.tsx so v2 panes drive the same
  // sidebar braille spinner / "Active" indicators as legacy. Two
  // signals feed the active-agents store:
  //   1. recordOutput(terminalId) on every grid change — the
  //      heartbeat-style "this pane just produced bytes" signal.
  //   2. detectWorkingSignal(rows) viewport scan — the stable
  //      "is a CLI LLM mid-request" hint ("esc to interrupt",
  //      "thinking…", etc.). Gated on displayOffset === 0 so a
  //      scrolled-up user can't pin the pane in 'working' state.
  // Idle transition fires from a 500ms interval that watches a
  // 1s grace window since the last working signal.
  const lastSeenWorkingAtRef = useRef<number>(0)

  // Process one snapshot/delta payload for activity-store updates.
  // Bumps the per-pane heartbeat unconditionally and runs the
  // working-signal viewport scan when the user isn't scrolled.
  const recordActivityFromSnapshot = useCallback(
    (snap: TermGridSnapshot) => {
      useActiveAgentsStore.getState().recordOutput(terminalId)
      if (snap.displayOffset !== 0) return
      // Build the row→{text} map detectWorkingSignal expects.
      // Only the bottom window matters (the function scans the
      // last `windowRows` rows), but the cost of building all
      // rows is dominated by the network payload anyway.
      const lines = new Map<number, { text: string }>()
      for (let r = 0; r < snap.grid.length; r++) {
        lines.set(r, { text: rowToText(snap.grid[r]) })
      }
      if (detectWorkingSignal(lines, snap.rows)) {
        lastSeenWorkingAtRef.current = Date.now()
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, true)
      }
    },
    [terminalId],
  )

  // ── Working-state idle watcher ────────────────────────────────
  // Working → idle transitions when no signal has been seen for
  // 1 s. Same 500 ms cadence as legacy so the transition is at
  // most ~1.5 s after the real one but never flickers on a
  // single-frame status-line gap.
  useEffect(() => {
    const IDLE_GRACE_MS = 1000
    const interval = setInterval(() => {
      const last = lastSeenWorkingAtRef.current
      if (last === 0) return
      if (Date.now() - last > IDLE_GRACE_MS) {
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
        lastSeenWorkingAtRef.current = 0
      }
    }, 500)
    return () => clearInterval(interval)
  }, [terminalId])

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
      perfLog('mount', spawnedAt
        ? { since_keystroke_ms: Math.round(performance.now() - spawnedAt) }
        : undefined)
      setPhase({ kind: 'spawning' })

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

      // Boot with retry. `Tauri auto-update → relaunch` produces a
      // ~2–5 s window where the renderer is back up but the daemon
      // is mid-restart (version-mismatch handshake from 0.35.0 kicks
      // it). Without retry, every v2 pane that mounts in that window
      // surfaces "spawn fetch failed: TypeError: Load failed" until
      // the user manually closes + reopens it. Legacy panes are
      // immune because they spawn in-process via Tauri IPC and never
      // hit the daemon HTTP socket; this retry brings v2 to parity.
      //
      // Strategy: retry on network-level failures and 5xx for up to
      // ~10 s with exponential backoff (250 → 500 → 1000 → 2000 ms,
      // capped at 2000). 4xx surfaces immediately — it's a real
      // request error, not a transient unreachability.
      const BOOT_DEADLINE_MS = 10_000
      const __t_boot_start = performance.now()
      let creds: { port: number; token: string } | null = null
      let spawn: {
        sessionId: string
        agentName: string
        cols: number
        rows: number
        reused: boolean
      } | null = null
      let attempt = 0
      while (true) {
        if (cancelled) return
        attempt += 1
        const __t_attempt = performance.now()
        try {
          if (!creds) {
            perfLog('creds_start', { attempt: String(attempt) })
            creds = await getDaemonWs()
            perfLog('creds_end', { elapsed_ms: (performance.now() - __t_attempt).toFixed(1) })
          }
          perfLog('spawn_fetch_start', { attempt: String(attempt) })
          const __t_spawn_fetch = performance.now()
          const spawnRes = await fetch(
            `http://127.0.0.1:${creds.port}/cli/sessions/v2/spawn?token=${creds.token}`,
            {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(spawnBody),
            },
          )
          if (spawnRes.status >= 500) {
            // Daemon answered but failed — likely mid-init right
            // after restart. Retryable.
            const body = await spawnRes.text().catch(() => '')
            invalidateDaemonWs()
            throw new Error(`spawn ${spawnRes.status}: ${body || 'no body'}`)
          }
          if (!spawnRes.ok) {
            // 4xx — genuine request error, surface immediately. Bad
            // body, missing field, etc. Won't get better by waiting.
            const body = await spawnRes.text()
            if (!cancelled) {
              setPhase({ kind: 'error', message: `spawn ${spawnRes.status}: ${body}` })
            }
            return
          }
          spawn = (await spawnRes.json()) as typeof spawn
          perfLog('spawn_fetch_end', {
            elapsed_ms: (performance.now() - __t_spawn_fetch).toFixed(1),
            reused: String(spawn!.reused),
            sid: spawn!.sessionId.slice(0, 8),
            attempt: String(attempt),
          })
          break
        } catch (e) {
          // Network errors (TypeError 'Load failed' from fetch when
          // socket is closed) and 5xx land here. Daemon-creds errors
          // also land here (Tauri command failed). All are retryable
          // until the deadline.
          invalidateDaemonWs()
          creds = null
          const elapsedTotalMs = performance.now() - __t_boot_start
          if (elapsedTotalMs > BOOT_DEADLINE_MS) {
            if (!cancelled) {
              setPhase({
                kind: 'error',
                message: `spawn failed after ${Math.round(elapsedTotalMs / 1000)}s: ${String(e)}`,
              })
            }
            return
          }
          // Exponential backoff capped at 2 s.
          const delayMs = Math.min(250 * 2 ** Math.min(attempt - 1, 3), 2000)
          perfLog('spawn_retry', {
            attempt: String(attempt),
            delay_ms: String(delayMs),
            elapsed_ms: Math.round(elapsedTotalMs).toString(),
            err: String(e).slice(0, 60),
          })
          await new Promise((r) => setTimeout(r, delayMs))
        }
      }

      if (!creds || !spawn) return // unreachable; satisfies TS
      firstSnapshotReusedRef.current = spawn.reused
      if (cancelled) return

      setPhase({ kind: 'connecting', sessionId: spawn.sessionId })

      perfLog('ws_opening')
      const __t_ws = performance.now()
      const ws = new WebSocket(
        `ws://127.0.0.1:${creds.port}/cli/sessions/grid?session=${spawn.sessionId}&token=${creds.token}`,
      )
      wsRef.current = ws

      ws.onopen = () => {
        perfLog('ws_open', { elapsed_ms: (performance.now() - __t_ws).toFixed(1) })
      }

      ws.onmessage = (evt) => {
        if (typeof evt.data !== 'string') return
        let parsed: OutboundMsg
        try {
          parsed = JSON.parse(evt.data) as OutboundMsg
        } catch {
          return
        }
        switch (parsed.event) {
          case 'snapshot': {
            const isFirst = !firstSnapshotSeenRef.current
            if (isFirst) {
              firstSnapshotSeenRef.current = true
              const empty = isGridEmpty(parsed.payload)
              firstSnapshotEmptyRef.current = empty
              perfLog('first_snapshot', {
                rows: parsed.payload.rows,
                cols: parsed.payload.cols,
                empty: String(empty),
                scrollback: parsed.payload.scrollback.length,
              })
            }
            setSnapshot(parsed.payload)
            recordActivityFromSnapshot(parsed.payload)
            setPhase({ kind: 'ready', sessionId: spawn.sessionId })
            break
          }
          case 'delta':
            setSnapshot((prev) => {
              const next = mergeDelta(prev, parsed.payload)
              if (next) recordActivityFromSnapshot(next)
              return next
            })
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
        if (cancelled) return
        // If we already received child_exit, the daemon initiated the
        // teardown and any onerror that follows is a concurrent TCP
        // close, not a real failure. Don't clobber the 'exited' state.
        setPhase((prev) =>
          prev.kind === 'exited' ? prev : { kind: 'error', message: 'ws error' },
        )
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

  // ── A7.5 perf: first_render + tui_first_paint + SUMMARY ──────
  // first_render fires once after `setSnapshot` causes a paint.
  // tui_first_paint fires once when the grid transitions from
  // empty → non-empty (cold spawn — child wrote its first bytes)
  // OR collapses with first_render when the initial snapshot was
  // already non-empty (reattach).
  useEffect(() => {
    if (!import.meta.env.DEV) return
    if (!snapshot) return

    if (!firstRenderFiredRef.current) {
      firstRenderFiredRef.current = true
      perfLog('first_render')
      const stages = stageMsRef.current
      const total = Math.round(
        performance.now() - (mountT0Ref.current ?? 0),
      )
      const reused = firstSnapshotReusedRef.current
      // eslint-disable-next-line no-console
      console.info(
        `[v2-perf] SUMMARY total_render_ms=${total} reused=${reused}` +
          ` mount=${Math.round(stages.mount ?? 0)}` +
          ` creds_end=${Math.round(stages.creds_end ?? 0)}` +
          ` spawn_fetch_end=${Math.round(stages.spawn_fetch_end ?? 0)}` +
          ` ws_open=${Math.round(stages.ws_open ?? 0)}` +
          ` first_snapshot=${Math.round(stages.first_snapshot ?? 0)}` +
          ` first_render=${Math.round(stages.first_render ?? 0)}`,
      )
      // Reattach scenario: initial snapshot already had content.
      // Collapse tui_first_paint with first_render.
      if (
        !firstSnapshotEmptyRef.current &&
        !tuiFirstPaintFiredRef.current
      ) {
        tuiFirstPaintFiredRef.current = true
        perfLog('tui_first_paint', { collapsed: 'true' })
        // eslint-disable-next-line no-console
        console.info(
          `[v2-perf] TUI_SUMMARY total_tui_ms=${total} reused=${reused} collapsed=true`,
        )
      }
    }

    // Cold spawn path: wait for the first non-empty grid update.
    if (
      !tuiFirstPaintFiredRef.current &&
      firstSnapshotEmptyRef.current &&
      !isGridEmpty(snapshot)
    ) {
      tuiFirstPaintFiredRef.current = true
      perfLog('tui_first_paint')
      const stages = stageMsRef.current
      const total = Math.round(
        performance.now() - (mountT0Ref.current ?? 0),
      )
      const renderToTui = Math.round(
        (stages.tui_first_paint ?? 0) - (stages.first_render ?? 0),
      )
      // eslint-disable-next-line no-console
      console.info(
        `[v2-perf] TUI_SUMMARY total_tui_ms=${total}` +
          ` reused=${firstSnapshotReusedRef.current}` +
          ` render_to_tui_ms=${renderToTui}`,
      )
    }
  }, [snapshot, perfLog])

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

  // Re-focus terminal when the OS window regains focus (e.g.,
  // switching back from another app). Only re-focuses if THIS
  // container held focus before the window blur — prevents
  // stealing focus from an input/textarea that happens to be
  // visible. Mirrors AlacrittyTerminalView.tsx's pattern.
  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    let wasFocused = false
    const onBlur = () => {
      wasFocused =
        document.activeElement === container ||
        container.contains(document.activeElement)
    }
    const onFocus = () => {
      if (!wasFocused) return
      requestAnimationFrame(() => container.focus())
    }
    window.addEventListener('blur', onBlur)
    window.addEventListener('focus', onFocus)
    return () => {
      window.removeEventListener('blur', onBlur)
      window.removeEventListener('focus', onFocus)
    }
  }, [])

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
      const text = e.clipboardData?.getData('text') ?? ''
      e.preventDefault()
      setViewportOffset(0)

      // Finder's Cmd+C copies file refs via NSFilenamesPboardType,
      // which WKWebView doesn't expose through the web clipboard
      // API. Query the native pasteboard: if file paths are
      // present, paste them shell-escaped (matching v1's drag-drop
      // behavior). Fall back to text paste otherwise.
      invoke<string[]>('clipboard_read_file_paths')
        .then((paths) => {
          if (paths && paths.length > 0) {
            sendInput(buildDropPayload(paths))
            return
          }
          if (text) sendInput(text)
        })
        .catch(() => {
          if (text) sendInput(text)
        })
    }

    el.addEventListener('keydown', onKey)
    el.addEventListener('paste', onPaste)
    el.focus()
    return () => {
      el.removeEventListener('keydown', onKey)
      el.removeEventListener('paste', onPaste)
    }
  }, [phase.kind, sendInput])

  // ── Compose visible rows ──────────────────────────────────────
  //
  // Declared before the link-detection handlers below because
  // `handleMouseMove` closes over `visibleRows` and JS temporal-
  // dead-zone rules reject the closure at render time if the
  // `const` is declared later. (Same class of fix as the
  // cellMetrics hoist that happened earlier in the Kessel-T0
  // work.)
  // Visible rows + their absolute (scrollback-anchored) row indices.
  // Keying the rendered row divs by absolute index — instead of by
  // visual 0..N position — keeps the same DOM node attached to the
  // same logical row across scrolls. The browser's text selection is
  // anchored to text nodes inside those divs; if the divs survive
  // (just move position), native selection follows the content as
  // expected. Without this, scrolling reused row divs with new
  // content and the highlight visually "stayed" while text moved.
  const { visibleRows, visibleRowAbsRows } = useMemo(() => {
    if (!snapshot) {
      return { visibleRows: [] as CellRun[][], visibleRowAbsRows: [] as number[] }
    }
    const { scrollback, grid, rows: r } = snapshot
    const totalLen = scrollback.length + grid.length
    const windowEnd = totalLen - viewportOffset
    const windowStart = windowEnd - r
    const rows: CellRun[][] = []
    const abs: number[] = []
    for (let i = 0; i < r; i++) {
      const a = windowStart + i
      abs.push(a)
      if (a < 0) rows.push([])
      else if (a < scrollback.length) rows.push(scrollback[a])
      else rows.push(grid[a - scrollback.length])
    }
    return { visibleRows: rows, visibleRowAbsRows: abs }
  }, [viewportOffset, snapshot])

  // ── Link detection: Cmd key tracking ──────────────────────────
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Meta') cmdHeldRef.current = true
    }
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'Meta') {
        cmdHeldRef.current = false
        if (linkClickMode === 'cmd-click') setHoveredLink(null)
      }
    }
    const onBlur = () => {
      cmdHeldRef.current = false
      setHoveredLink(null)
    }
    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', onBlur)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onBlur)
    }
  }, [linkClickMode])

  // ── Link detection: hover → {row, link} state ─────────────────
  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (linkClickMode === 'cmd-click' && !cmdHeldRef.current) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      // Throttle: skip if mouse moved < 4px and < 80ms since last.
      const now = Date.now()
      const dx = e.clientX - lastDetectPosRef.current.x
      const dy = e.clientY - lastDetectPosRef.current.y
      if (dx * dx + dy * dy < 16 && now - lastDetectTimeRef.current < 80) return
      lastDetectPosRef.current = { x: e.clientX, y: e.clientY }
      lastDetectTimeRef.current = now

      const el = containerRef.current
      if (!el || !snapshot) return
      const rect = el.getBoundingClientRect()
      const { width: cw, height: ch } = cellMetrics
      if (cw === 0 || ch === 0) return
      // The 4px padding on the container biases cell positions —
      // subtract before dividing.
      const row = Math.floor((e.clientY - rect.top - 4) / ch)
      const col = Math.floor((e.clientX - rect.left - 4) / cw)
      const visibleRow = visibleRows[row]
      if (!visibleRow) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      const text = rowToText(visibleRow)
      if (!text.trim()) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      const links = detectLinks(text, cwd)
      const hit = links.find((l) => col >= l.start && col < l.end)
      if (hit) {
        if (
          !hoveredLink ||
          hoveredLink.row !== row ||
          hoveredLink.link.start !== hit.start
        ) {
          setHoveredLink({ row, link: hit })
        }
      } else if (hoveredLink) {
        setHoveredLink(null)
      }
    },
    [linkClickMode, hoveredLink, cellMetrics, snapshot, visibleRows, cwd],
  )

  const handleMouseLeave = useCallback(() => {
    if (hoveredLink) setHoveredLink(null)
  }, [hoveredLink])

  const handleMouseDown = useCallback(() => {
    mouseDownLinkRef.current = hoveredLink?.link ?? null
  }, [hoveredLink])

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      if (linkClickMode === 'cmd-click' && !e.metaKey) return
      if (!hoveredLink) return
      // Validate: mouse-down must have been on the same link so a
      // drag-to-link doesn't false-click.
      const downLink = mouseDownLinkRef.current
      mouseDownLinkRef.current = null
      if (
        !downLink ||
        downLink.start !== hoveredLink.link.start ||
        downLink.target !== hoveredLink.link.target
      ) {
        return
      }

      const clicked = hoveredLink.link
      e.preventDefault()
      e.stopPropagation()

      if (clicked.type === 'url') {
        invoke('open_external', { url: clicked.target }).catch((err) =>
          console.warn('[terminal-v2/link]', err),
        )
      } else if (clicked.type === 'file' && clicked.filePath) {
        const tabsStore = useTabsStore.getState()
        const openInSplit =
          useTerminalSettingsStore.getState().openLinksInSplitPane

        if (openInSplit && tabId && paneGroupId) {
          const tab = tabsStore.tabs.find((t) => t.id === tabId)
          if (tab && tab.paneGroups.size > 1) {
            const siblingId = [...tab.paneGroups.keys()].find(
              (id) => id !== paneGroupId,
            )
            if (siblingId) {
              tabsStore.openFileInPaneGroup(tabId, siblingId, clicked.filePath)
              return
            }
          }
        }
        tabsStore.openFileInNewTab(clicked.filePath)
      }
    },
    [linkClickMode, hoveredLink, tabId, paneGroupId],
  )

  // ── Drag + drop of files (from Finder or K2SO files tab) ──────
  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }, [])

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const files = e.dataTransfer.files
      if (files.length > 0) {
        const paths: string[] = []
        for (let i = 0; i < files.length; i++) {
          // Tauri exposes full path via .path (non-standard field).
          const p = (files[i] as unknown as { path?: string }).path
          if (p) paths.push(p)
        }
        if (paths.length > 0) {
          sendInput(buildDropPayload(paths))
          return
        }
      }
      const text = e.dataTransfer.getData('text/plain')
      if (text) sendInput(text)
    },
    [sendInput],
  )

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

  // Container cursor hints at link-clickability without rewriting
  // the row DOM (simpler than overlaying underlines per hovered
  // link). Matches v1's affordance.
  const finalContainerStyle: React.CSSProperties = {
    ...containerStyle,
    cursor: hoveredLink ? 'pointer' : 'text',
  }

  return (
    <div
      ref={containerRef}
      className="alacritty-v2-pane"
      data-session-id={debugSessionId}
      // App.tsx's global click + refocus-poll use these two data
      // attributes to find the active terminal and keep it focused
      // after (a) clicks on blank canvas, (b) Cmd+K / Cmd+L
      // palette close, (c) any overlay Esc-out. Matches v1.
      data-terminal-container=""
      data-terminal-visible="true"
      tabIndex={0}
      style={finalContainerStyle}
      onMouseMove={handleMouseMove}
      onMouseLeave={handleMouseLeave}
      onMouseDown={handleMouseDown}
      onClick={handleClick}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {visibleRows.map((row, rowIdx) => {
        const absRow = visibleRowAbsRows[rowIdx] ?? rowIdx
        return (
          <div key={`abs-${absRow}`}>{renderRowRuns(row, absRow)}</div>
        )
      })}
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

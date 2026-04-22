// Kessel — tab-pane wrapper that bridges the existing Terminal tab
// model to a Session Stream view. The sibling `AlacrittyTerminalView`
// uses `invoke('terminal_create', ...)` on mount to lazy-create its
// PTY; this component does the equivalent for Kessel: POST to
// `/cli/sessions/spawn` and feed the returned sessionId into a
// SessionStreamView.
//
// Lifecycle:
//   1. On mount, `daemon_ws_url` → {port, token}.
//   2. POST /cli/sessions/spawn with the tab's cwd + command.
//   3. Store the returned sessionId locally. (Could also be persisted
//      on the tab via a setter; deferred — each Tauri-app session has
//      a fresh daemon session, so no cross-restart persistence yet.)
//   4. Mount SessionStreamView with the sessionId.
//
// Errors surface via a simple message overlay so the user sees why
// the pane is blank instead of a silent white box.

import React, { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { SessionStreamView } from './SessionStreamView'
import { invalidateDaemonWs } from './daemon-ws'

export interface KesselTerminalProps {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  fontSize?: number
  onExit?: (code: number) => void
}

/** Shape returned by the `kessel_spawn` Tauri command. The Rust side
 *  does the HTTP POST to the daemon's `/cli/sessions/spawn`, reuses
 *  a persistent reqwest::Client with keep-alive, and hands us the
 *  whole triple (sessionId, port, token) in one IPC hop so the
 *  browser never pays the fetch overhead.
 *
 *  `timingUs` breaks down the spawn cost so dev-mode logging can
 *  show where the milliseconds are going. Units are microseconds. */
interface KesselSpawnResult {
  sessionId: string
  agentName: string
  port: number
  token: string
  spawnMs: number
  timingUs: {
    credsUs: number
    serializeUs: number
    httpUs: number
    responseReadUs: number
    deserializeUs: number
  }
}

type State =
  | { kind: 'idle' }
  | { kind: 'spawning' }
  | { kind: 'ready'; port: number; token: string; sessionId: string }
  | { kind: 'error'; message: string }

export function KesselTerminal(props: KesselTerminalProps): React.JSX.Element {
  const { terminalId, cwd, command, args, fontSize } = props
  const [state, setState] = useState<State>({ kind: 'idle' })

  useEffect(() => {
    let cancelled = false
    const bootT0 = performance.now()

    async function boot() {
      setState({ kind: 'spawning' })
      // eslint-disable-next-line no-console
      console.info(
        `%c[Kessel] spawning terminal tab-${terminalId}`,
        'color:#0ff;font-weight:bold',
        { cwd, command, args },
      )
      performance.mark(`kessel:boot:${terminalId}:start`)

      // L1.4 — one Tauri IPC that:
      //   (a) reads cached daemon port/token from Rust-side cache
      //       (no repeated disk I/O)
      //   (b) POSTs to /cli/sessions/spawn via a persistent
      //       reqwest::blocking::Client with HTTP keep-alive
      //   (c) returns {sessionId, port, token} in one hop
      // Replaces the prior [invoke daemon_ws_url → browser fetch →
      // await .json()] waterfall with a single round trip.
      let result: KesselSpawnResult
      try {
        result = await invoke<KesselSpawnResult>('kessel_spawn', {
          req: {
            terminalId,
            cwd,
            command: command ?? null,
            args: args ?? null,
            cols: 80,
            rows: 24,
          },
        })
      } catch (e) {
        // Invalidate the in-browser daemon-ws cache too so the next
        // fallback call (HarnessLab etc.) re-reads creds from disk.
        invalidateDaemonWs()
        if (!cancelled) {
          setState({ kind: 'error', message: `spawn error: ${String(e)}` })
        }
        return
      }

      if (!cancelled) {
        performance.mark(`kessel:boot:${terminalId}:spawned`)
        try {
          performance.measure(
            `kessel:boot:${terminalId}:total`,
            `kessel:boot:${terminalId}:start`,
            `kessel:boot:${terminalId}:spawned`,
          )
        } catch {
          /* perf measure failures don't matter */
        }
        const totalMs = Math.round(performance.now() - bootT0)
        const t = result.timingUs
        // eslint-disable-next-line no-console
        console.info(
          `%c[Kessel] ready tab-${terminalId} total=${totalMs}ms rust=${result.spawnMs}ms ` +
            `(creds=${Math.round(t.credsUs / 1000)}ms ` +
            `ser=${(t.serializeUs / 1000).toFixed(1)}ms ` +
            `http=${Math.round(t.httpUs / 1000)}ms ` +
            `resp=${(t.responseReadUs / 1000).toFixed(1)}ms ` +
            `de=${(t.deserializeUs / 1000).toFixed(1)}ms)`,
          'color:#0ff',
        )
        setState({
          kind: 'ready',
          port: result.port,
          token: result.token,
          sessionId: result.sessionId,
        })
      }
    }

    void boot()
    return () => {
      cancelled = true
    }
    // Re-spawn only when terminalId changes — same terminal tab
    // keeps its session across prop tweaks.
  }, [terminalId, cwd, command, args?.join('\0')])

  if (state.kind === 'error') {
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
        {state.message}
      </div>
    )
  }

  // L1.5 — optimistic mount. Render SessionStreamView in both
  // `spawning` and `ready` states; during `spawning` we pass
  // sessionId=null so the WS effect skips. Font measurement, grid
  // allocation, cursor styling, and layout all complete during the
  // spawn wait, so when the Rust-side spawn returns and we swap
  // sessionId from null → real, the only thing that changes is the
  // WS connection starting — the pane is visually already there.
  const isReady = state.kind === 'ready'
  return (
    <SessionStreamView
      sessionId={isReady ? state.sessionId : null}
      port={isReady ? state.port : 0}
      token={isReady ? state.token : ''}
      cols={80}
      rows={24}
      fontSize={fontSize}
      autoResize
      interactive
    />
  )
}

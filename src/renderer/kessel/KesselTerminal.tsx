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
import { SessionStreamView } from './SessionStreamView'
import { getDaemonWs, invalidateDaemonWs } from './daemon-ws'

export interface KesselTerminalProps {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  fontSize?: number
  onExit?: (code: number) => void
}

interface SpawnResult {
  sessionId: string
  agentName: string
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
      // Perf timing — lets us compare before/after optimization runs.
      // Shows up in devtools Performance panel as named marks.
      performance.mark(`kessel:boot:${terminalId}:start`)
      // 1. Look up daemon port + token via the shared cache. First
      //    call in the app session hits the Tauri command (which
      //    reads 2 files from disk); every subsequent call is a
      //    cheap promise return. Prewarmed at app startup so by the
      //    time the user opens the first terminal it's already
      //    resolved.
      let ws: { port: number; token: string }
      try {
        ws = await getDaemonWs()
      } catch (e) {
        if (!cancelled) {
          setState({ kind: 'error', message: String(e) })
        }
        return
      }
      performance.mark(`kessel:boot:${terminalId}:daemon-ws-ready`)
      // 2. Spawn via daemon. Use terminalId as the agent_name so
      //    live lookups via /cli/agents/running can find this pane.
      let result: SpawnResult
      try {
        const res = await fetch(
          `http://127.0.0.1:${ws.port}/cli/sessions/spawn?token=${ws.token}`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              agent_name: `tab-${terminalId}`,
              cwd,
              command,
              args,
              cols: 80,
              rows: 24,
            }),
          },
        )
        if (!res.ok) {
          const body = await res.text()
          // 401/403 likely mean the cached token is stale — daemon
          // may have restarted. Invalidate so the next spawn retries.
          if (res.status === 401 || res.status === 403) {
            invalidateDaemonWs()
          }
          if (!cancelled) {
            setState({
              kind: 'error',
              message: `spawn failed: HTTP ${res.status} ${body}`,
            })
          }
          return
        }
        result = (await res.json()) as SpawnResult
      } catch (e) {
        // Network-level failure (daemon down, wrong port). Invalidate
        // so the retry path re-reads the credential files.
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
        // eslint-disable-next-line no-console
        console.info(
          `%c[Kessel] ready tab-${terminalId} in ${Math.round(performance.now() - bootT0)}ms`,
          'color:#0ff',
        )
        setState({
          kind: 'ready',
          port: ws.port,
          token: ws.token,
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

  if (state.kind !== 'ready') {
    // Blank pane in the Kessel default background color — visually
    // identical to an empty live pane, so the transition to the
    // real SessionStreamView feels instant instead of a gray→black
    // flash. Matches config.colors.background in KesselConfig.
    return (
      <div
        style={{
          width: '100%',
          height: '100%',
          flex: 1,
          backgroundColor: 'rgb(10,10,10)',
        }}
      />
    )
  }

  return (
    <SessionStreamView
      sessionId={state.sessionId}
      port={state.port}
      token={state.token}
      cols={80}
      rows={24}
      fontSize={fontSize}
      autoResize
      interactive
    />
  )
}

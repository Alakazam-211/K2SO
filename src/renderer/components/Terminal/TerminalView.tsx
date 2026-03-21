import { useEffect, useRef, useCallback } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import { trpc } from '@/lib/trpc'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import '@xterm/xterm/css/xterm.css'

interface TerminalViewProps {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  onExit?: (exitCode: number) => void
}

export function TerminalView({
  terminalId,
  cwd,
  command,
  args,
  onExit
}: TerminalViewProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const xtermRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const ptyIdRef = useRef<string | null>(null)
  const isExitedRef = useRef(false)

  // Stable fit function
  const doFit = useCallback(() => {
    const fitAddon = fitAddonRef.current
    const xterm = xtermRef.current
    if (!fitAddon || !xterm || !containerRef.current) return

    try {
      fitAddon.fit()

      const ptyId = ptyIdRef.current
      if (ptyId && xterm.cols > 0 && xterm.rows > 0) {
        trpc.terminal.resize.mutate({
          id: ptyId,
          cols: xterm.cols,
          rows: xterm.rows
        })
      }
    } catch {
      // Ignore fit errors during teardown
    }
  }, [])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    // ── Create xterm instance ─────────────────────────────────────────
    const xterm = new Terminal({
      theme: {
        background: '#0a0a0a',
        foreground: '#e0e0e0',
        cursor: '#528bff',
        selectionBackground: '#3b82f640',
        black: '#1e1e1e',
        red: '#f44747',
        green: '#6a9955',
        yellow: '#d7ba7d',
        blue: '#528bff',
        magenta: '#c586c0',
        cyan: '#4ec9b0',
        white: '#e0e0e0',
        brightBlack: '#5a5a5a',
        brightRed: '#f44747',
        brightGreen: '#6a9955',
        brightYellow: '#d7ba7d',
        brightBlue: '#528bff',
        brightMagenta: '#c586c0',
        brightCyan: '#4ec9b0',
        brightWhite: '#ffffff'
      },
      fontFamily: "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
      fontSize: useTerminalSettingsStore.getState().fontSize,
      lineHeight: 1.2,
      scrollback: 5000,
      macOptionIsMeta: false,
      cursorBlink: true,
      cursorStyle: 'bar',
      allowProposedApi: true
    })

    xtermRef.current = xterm

    // ── Fit addon ─────────────────────────────────────────────────────
    const fitAddon = new FitAddon()
    fitAddonRef.current = fitAddon
    xterm.loadAddon(fitAddon)

    // ── Open in container ─────────────────────────────────────────────
    xterm.open(container)

    // ── Try WebGL, fallback to canvas/DOM ─────────────────────────────
    try {
      const webglAddon = new WebglAddon()
      webglAddon.onContextLoss(() => {
        webglAddon.dispose()
      })
      xterm.loadAddon(webglAddon)
    } catch {
      // WebGL not available, xterm falls back to canvas renderer
    }

    // ── Initial fit + focus ────────────────────────────────────────────
    requestAnimationFrame(() => {
      doFit()
      xterm.focus()
    })

    // ── Resize observer ───────────────────────────────────────────────
    const resizeObserver = new ResizeObserver(() => {
      doFit()
    })
    resizeObserver.observe(container)

    // ── Spawn the pty ─────────────────────────────────────────────────
    let dataUnsub: (() => void) | undefined
    let exitUnsub: (() => void) | undefined

    const setup = async (): Promise<void> => {
      try {
        const result = await trpc.terminal.create.mutate({
          cwd,
          command,
          args
        })

        ptyIdRef.current = result.id

        // Forward keystrokes to pty
        const disposable = xterm.onData((data) => {
          if (ptyIdRef.current && !isExitedRef.current) {
            trpc.terminal.write.mutate({ id: ptyIdRef.current, data })
          }
        })

        // Subscribe to pty output
        dataUnsub = trpc.terminal.onData.subscribe(
          { id: result.id },
          {
            onData(data: string) {
              xterm.write(data)
            },
            onError(err) {
              xterm.writeln(`\r\n\x1b[31m[Connection error: ${err.message}]\x1b[0m`)
            }
          }
        ).unsubscribe

        // Subscribe to pty exit
        exitUnsub = trpc.terminal.onExit.subscribe(
          { id: result.id },
          {
            onData(data: { exitCode: number; signal?: number }) {
              isExitedRef.current = true
              xterm.writeln(
                `\r\n\x1b[90m[Process exited with code ${data.exitCode}]\x1b[0m`
              )
              onExit?.(data.exitCode)
            },
            onError() {
              // Ignore
            }
          }
        ).unsubscribe

        // Do an initial resize now that the pty is spawned
        if (xterm.cols > 0 && xterm.rows > 0) {
          trpc.terminal.resize.mutate({
            id: result.id,
            cols: xterm.cols,
            rows: xterm.rows
          })
        }

        // Store disposable ref for cleanup
        // (disposed in the effect cleanup via xterm.dispose() which handles all addons)
      } catch (err) {
        xterm.writeln(
          `\r\n\x1b[31m[Failed to create terminal: ${err instanceof Error ? err.message : 'unknown error'}]\x1b[0m`
        )
      }
    }

    setup()

    // ── Cleanup ───────────────────────────────────────────────────────
    return () => {
      resizeObserver.disconnect()
      dataUnsub?.()
      exitUnsub?.()

      const ptyId = ptyIdRef.current
      if (ptyId && !isExitedRef.current) {
        trpc.terminal.kill.mutate({ id: ptyId })
      }

      xterm.dispose()
      xtermRef.current = null
      fitAddonRef.current = null
      ptyIdRef.current = null
    }
  }, [terminalId]) // Only re-run if terminal ID changes

  // ── Subscribe to terminal font size changes ────────────────────────
  useEffect(() => {
    let prevFontSize = useTerminalSettingsStore.getState().fontSize

    const unsubscribe = useTerminalSettingsStore.subscribe((state) => {
      if (state.fontSize === prevFontSize) return
      prevFontSize = state.fontSize

      const xterm = xtermRef.current
      const fitAddon = fitAddonRef.current
      if (!xterm || !fitAddon) return

      xterm.options.fontSize = state.fontSize
      try {
        fitAddon.fit()
        const ptyId = ptyIdRef.current
        if (ptyId && xterm.cols > 0 && xterm.rows > 0) {
          trpc.terminal.resize.mutate({
            id: ptyId,
            cols: xterm.cols,
            rows: xterm.rows
          })
        }
      } catch {
        // Ignore fit errors
      }
    })

    return unsubscribe
  }, [])

  return (
    <div
      ref={containerRef}
      className="h-full w-full bg-[#0a0a0a] no-drag"
      style={{ padding: '4px 0 0 4px' }}
    />
  )
}

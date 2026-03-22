import { useEffect, useRef, useCallback } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { RESUMABLE_CLI_TOOLS } from '@shared/constants'
import '@xterm/xterm/css/xterm.css'

// ── Natural Text Editing ─────────────────────────────────────────────
// Maps macOS-native editing shortcuts to readline/terminal escape sequences.
// Mirrors iTerm2's "Natural Text Editing" preset.
//
// The handler intercepts keydown events before xterm processes them.
// It sends the appropriate escape sequence via xterm.input() and
// returns false to prevent xterm's default handling.

function setNaturalTextEditing(xterm: Terminal, enabled: boolean): void {
  if (!enabled) {
    // Pass-through handler — disables natural editing
    xterm.attachCustomKeyEventHandler(() => true)
    return
  }

  xterm.attachCustomKeyEventHandler((event: KeyboardEvent): boolean => {
    if (event.type !== 'keydown') return true

    const { key, metaKey, altKey } = event

    // Opt+Left → ESC b (move back one word)
    if (altKey && !metaKey && key === 'ArrowLeft') {
      xterm.input('\x1bb')
      return false
    }

    // Opt+Right → ESC f (move forward one word)
    if (altKey && !metaKey && key === 'ArrowRight') {
      xterm.input('\x1bf')
      return false
    }

    // Cmd+Left → Ctrl+A (beginning of line)
    if (metaKey && !altKey && key === 'ArrowLeft') {
      xterm.input('\x01')
      return false
    }

    // Cmd+Right → Ctrl+E (end of line)
    if (metaKey && !altKey && key === 'ArrowRight') {
      xterm.input('\x05')
      return false
    }

    // Opt+Backspace → Ctrl+W (delete word backward)
    if (altKey && !metaKey && key === 'Backspace') {
      xterm.input('\x17')
      return false
    }

    // Cmd+Backspace → Ctrl+U (delete to beginning of line)
    if (metaKey && !altKey && key === 'Backspace') {
      xterm.input('\x15')
      return false
    }

    // Opt+Delete → ESC d (delete word forward)
    if (altKey && !metaKey && key === 'Delete') {
      xterm.input('\x1bd')
      return false
    }

    return true
  })
}

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

  // Stable fit function — debounced to prevent flooding the PTY with
  // SIGWINCH signals during drag-resize, which causes TUI apps (Claude Code,
  // Gemini CLI, etc.) to partially redraw and leave box-drawing artifacts.
  const fitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const lastColsRef = useRef(0)
  const lastRowsRef = useRef(0)

  const doFit = useCallback(() => {
    if (fitTimerRef.current) clearTimeout(fitTimerRef.current)
    fitTimerRef.current = setTimeout(() => {
      const fitAddon = fitAddonRef.current
      const xterm = xtermRef.current
      if (!fitAddon || !xterm || !containerRef.current) return

      try {
        fitAddon.fit()

        // Only send resize to PTY if dimensions actually changed
        const ptyId = ptyIdRef.current
        if (
          ptyId &&
          xterm.cols > 0 &&
          xterm.rows > 0 &&
          (xterm.cols !== lastColsRef.current || xterm.rows !== lastRowsRef.current)
        ) {
          lastColsRef.current = xterm.cols
          lastRowsRef.current = xterm.rows
          invoke('terminal_resize', {
            id: ptyId,
            cols: xterm.cols,
            rows: xterm.rows
          })
        }
      } catch {
        // Ignore fit errors during teardown
      }
    }, 80)
  }, [])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    // Track whether xterm renderer is ready for writes
    let rendererReady = false
    const pendingWrites: string[] = []

    const safeWrite = (data: string): void => {
      if (rendererReady) {
        xterm.write(data)
      } else {
        pendingWrites.push(data)
      }
    }

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
      fontFamily: "'MesloLGM Nerd Font', 'MesloLGS Nerd Font', 'Hack Nerd Font', 'FiraCode Nerd Font', 'JetBrainsMono Nerd Font', 'Symbols Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
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

    // ── Unicode 11 for proper wide/ambiguous character widths ────────
    const unicode11 = new Unicode11Addon()
    xterm.loadAddon(unicode11)
    xterm.unicode.activeVersion = '11'

    // ── Natural text editing (macOS-style word/line navigation) ──────
    setNaturalTextEditing(xterm, useSettingsStore.getState().terminal.naturalTextEditing)

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
    // Wait for the renderer to initialize before fitting. When multiple
    // terminals are created simultaneously (e.g., via workspace_arrange),
    // the renderer may not be ready immediately.
    const fitWithRetry = (): void => {
      try {
        fitAddon.fit()
        xterm.focus()
        // Renderer is ready — flush any buffered writes
        rendererReady = true
        for (const data of pendingWrites) {
          xterm.write(data)
        }
        pendingWrites.length = 0
      } catch {
        // Renderer not ready yet — retry after a short delay
        setTimeout(fitWithRetry, 50)
      }
    }
    requestAnimationFrame(fitWithRetry)

    // ── Resize observer ───────────────────────────────────────────────
    const resizeObserver = new ResizeObserver(() => {
      doFit()
    })
    resizeObserver.observe(container)

    // ── Drop files into terminal → paste path ─────────────────────────
    // Handles native drags from Finder and from FileTree (via Tauri drag plugin).
    // Paths are shell-escaped so spaces/special chars work correctly.
    let dropUnlisten: UnlistenFn | undefined
    listen<{ paths: string[]; position: { x: number; y: number } }>(
      'tauri://drag-drop',
      (event) => {
        const { paths, position } = event.payload
        if (!paths || paths.length === 0) return
        if (!ptyIdRef.current || isExitedRef.current) return

        // Only handle if the drop landed on THIS terminal's container
        const el = document.elementFromPoint(position.x, position.y)
        if (!el || !container.contains(el)) return

        // Shell-escape each path and join with spaces
        const escaped = paths.map((p) => {
          // Wrap in single quotes; escape any existing single quotes
          if (/[^a-zA-Z0-9_\-./]/.test(p)) {
            return "'" + p.replace(/'/g, "'\\''") + "'"
          }
          return p
        })
        const text = escaped.join(' ')

        invoke('terminal_write', { id: ptyIdRef.current, data: text }).catch(() => {})
      }
    ).then((fn) => { dropUnlisten = fn })

    // ── Spawn or reattach the pty ──────────────────────────────────────
    let dataUnlisten: UnlistenFn | undefined
    let exitUnlisten: UnlistenFn | undefined

    const setup = async (): Promise<void> => {
      try {
        // Check if the terminal already exists (reattaching after tab switch)
        const alreadyExists = await invoke<boolean>('terminal_exists', { id: terminalId })

        let ptyId: string

        if (alreadyExists) {
          // Reattach: replay buffered output, then subscribe to live events
          ptyId = terminalId
          ptyIdRef.current = ptyId

          try {
            const buffer = await invoke<string>('terminal_get_buffer', { id: ptyId })
            if (buffer) {
              safeWrite(buffer)
            }
          } catch {
            // Buffer might not be available — that's OK
          }
        } else {
          // New terminal: get initial dimensions and create
          let initialCols: number | undefined
          let initialRows: number | undefined
          try {
            const dims = fitAddon.proposeDimensions()
            if (dims) {
              initialCols = dims.cols
              initialRows = dims.rows
            }
          } catch {
            // proposeDimensions can fail if container isn't laid out yet
          }

          const result = await invoke<{ id: string }>('terminal_create', {
            id: terminalId,
            cwd,
            command,
            args,
            cols: initialCols,
            rows: initialRows
          })

          ptyId = result.id
          ptyIdRef.current = ptyId
        }

        // Forward keystrokes to pty
        xterm.onData((data) => {
          if (ptyIdRef.current && !isExitedRef.current) {
            invoke('terminal_write', { id: ptyIdRef.current, data }).catch((err) => {
              // PTY may have died — mark as exited to stop further writes
              if (String(err).includes('not found') || String(err).includes('Write error')) {
                isExitedRef.current = true
              }
              console.error('[terminal] Write failed:', err)
            })
          }
        })

        // Subscribe to pty output via Tauri events
        dataUnlisten = await listen<string>(`terminal:data:${ptyId}`, (event) => {
          safeWrite(event.payload)
        })

        // Subscribe to pty exit via Tauri events
        exitUnlisten = await listen<{ exitCode: number; signal?: number }>(`terminal:exit:${ptyId}`, (event) => {
          isExitedRef.current = true
          xterm.writeln(
            `\r\n\x1b[90m[Process exited with code ${event.payload.exitCode}]\x1b[0m`
          )
          onExit?.(event.payload.exitCode)
        })

        // After reattach, do a fit+resize so the PTY knows the current dimensions
        if (alreadyExists) {
          requestAnimationFrame(() => {
            try {
              fitAddon.fit()
              if (ptyIdRef.current && xterm.cols > 0 && xterm.rows > 0) {
                invoke('terminal_resize', {
                  id: ptyIdRef.current,
                  cols: xterm.cols,
                  rows: xterm.rows,
                })
              }
            } catch {
              // Ignore fit errors
            }
          })
        }
      } catch (err) {
        xterm.writeln(
          `\r\n\x1b[31m[Failed to create terminal: ${err instanceof Error ? err.message : 'unknown error'}]\x1b[0m`
        )
      }
    }

    setup()

    // ── Cleanup ───────────────────────────────────────────────────────
    // On unmount, we dispose the xterm UI but do NOT kill the PTY.
    // The PTY continues running in the background and buffers output.
    // When this component remounts (tab re-activated), it reattaches.
    return () => {
      resizeObserver.disconnect()
      if (fitTimerRef.current) clearTimeout(fitTimerRef.current)
      dropUnlisten?.()
      dataUnlisten?.()
      exitUnlisten?.()

      // Only kill the PTY if the process has already exited.
      // Living PTYs are kept alive for reattachment.
      const ptyId = ptyIdRef.current
      if (ptyId && isExitedRef.current) {
        invoke('terminal_kill', { id: ptyId })
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
          invoke('terminal_resize', {
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

  // ── Subscribe to natural text editing setting changes ──────────────
  useEffect(() => {
    let prev = useSettingsStore.getState().terminal.naturalTextEditing

    const unsubscribe = useSettingsStore.subscribe((state) => {
      const next = state.terminal.naturalTextEditing
      if (next === prev) return
      prev = next

      const xterm = xtermRef.current
      if (!xterm) return
      setNaturalTextEditing(xterm, next)
    })

    return unsubscribe
  }, [])

  // ── Auto-detect chat title for CLI tool sessions ──────────────────
  // When running a resumable CLI tool (claude, cursor-agent), poll the
  // chat history for a session that started AFTER this terminal was
  // created. This avoids picking up the previous session's title.
  useEffect(() => {
    if (!command || !RESUMABLE_CLI_TOOLS[command]) return

    const provider = RESUMABLE_CLI_TOOLS[command].provider
    const createdAt = Date.now()
    let stopped = false
    let titleFound = false

    const poll = async (): Promise<void> => {
      if (stopped || titleFound) return
      try {
        const sessions = await invoke<Array<{
          sessionId: string
          title: string
          provider: string
          timestamp: number
        }>>(
          'chat_history_list_for_project',
          { projectPath: cwd }
        )

        // Find a session for this provider that started after the terminal was created
        const newSession = sessions.find((s) =>
          s.provider === provider && s.timestamp >= createdAt && s.title && s.title.length > 0
        )

        if (newSession) {
          // Find the tab that owns this terminal and update its title
          const store = useTabsStore.getState()
          const allTabs = [
            ...store.tabs,
            ...store.extraGroups.flatMap((g) => g.tabs)
          ]
          for (const tab of allTabs) {
            for (const [, pg] of tab.paneGroups) {
              for (const item of pg.items) {
                if (item.type === 'terminal' && (item.data as { terminalId: string }).terminalId === terminalId) {
                  store.setTabTitle(tab.id, newSession.title)
                  titleFound = true
                  return
                }
              }
            }
          }
        }
      } catch {
        // Chat history not available
      }

      if (!stopped && !titleFound) {
        setTimeout(poll, 3000)
      }
    }

    // Start polling after a delay (give the CLI tool time to start and write its first message)
    const timer = setTimeout(poll, 5000)
    return () => { stopped = true; clearTimeout(timer) }
  }, [command, cwd, terminalId])

  return (
    <div
      ref={containerRef}
      data-terminal-id={ptyIdRef.current || terminalId}
      className="h-full w-full bg-[#0a0a0a] no-drag"
      style={{ padding: '4px 0 0 4px' }}
    />
  )
}

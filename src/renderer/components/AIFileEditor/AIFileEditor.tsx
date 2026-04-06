import { useEffect, useRef, useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { AlacrittyTerminalView } from '../Terminal/AlacrittyTerminalView'

// ── Session file helpers ────────────────────────────────────────────

const SESSION_FILE = '.last_editor_session'

async function readEditorSession(cwd: string): Promise<string | null> {
  try {
    const result = await invoke<{ content: string }>('fs_read_file', {
      path: `${cwd}/${SESSION_FILE}`,
    })
    const id = result.content.trim()
    return id || null
  } catch {
    return null
  }
}

async function saveEditorSession(cwd: string, command: string | undefined): Promise<void> {
  if (!command) return
  // Map command to provider name for session detection
  const provider = command === 'claude' ? 'claude' : command === 'cursor' ? 'cursor' : null
  if (!provider) return

  try {
    const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
      provider,
      projectPath: cwd,
    })
    if (sessionId) {
      await invoke('fs_write_file', {
        path: `${cwd}/${SESSION_FILE}`,
        content: sessionId,
      })
    }
  } catch {
    // Non-fatal — session save is best-effort
  }
}

// ── Props ────────────────────────────────────────────────────────────

interface AIFileEditorProps {
  /** Absolute path to the file being edited */
  filePath: string
  /** Directory to watch for changes (usually the parent of filePath) */
  watchDir: string
  /** Terminal working directory */
  cwd: string
  /** Command to auto-run in the terminal (e.g. 'claude') */
  command?: string
  /** Arguments for the command */
  args?: string[]
  /** Context shown above the terminal (what this editor is for) */
  instructions?: string
  /** Warning text for the banner (default provided) */
  warningText?: string
  /** Live preview content — any React node, managed by the parent */
  preview: React.ReactNode
  /** Called when the watched file changes. Parent handles parsing. */
  onFileChange: (content: string) => void
  /** Called when user clicks Back */
  onClose: () => void
  /** Top bar title */
  title?: string
  /** Optional manual refresh callback */
  onManualRefresh?: () => void
  /** Enable file rename detection — watcher scans watchDir for renamed files (default: false) */
  trackFileRename?: boolean
}

// ── Component ────────────────────────────────────────────────────────

export function AIFileEditor({
  filePath,
  watchDir,
  cwd,
  command,
  args,
  instructions,
  warningText = 'This is a real terminal with full system access. It can edit files beyond this one if given the wrong command.',
  preview,
  onFileChange,
  onClose,
  title = 'AI File Editor',
  onManualRefresh,
  trackFileRename = false,
}: AIFileEditorProps): React.JSX.Element {
  const terminalIdRef = useRef(`ai-editor-${crypto.randomUUID()}`)
  const [terminalReady, setTerminalReady] = useState(false)
  const [activeFilePath, setActiveFilePath] = useState(filePath)

  // Sync with prop changes (e.g. persona editor tab switches)
  useEffect(() => {
    setActiveFilePath(filePath)
    lastContentRef.current = '' // Reset so new file content triggers onFileChange
  }, [filePath])
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // ── Session resume: detect previous editor session ────────────────
  const [resolvedArgs, setResolvedArgs] = useState<string[] | undefined>(undefined)
  const [argsReady, setArgsReady] = useState(false)
  const commandRef = useRef(command)
  commandRef.current = command

  useEffect(() => {
    let cancelled = false
    const resolve = async () => {
      if (!command || command !== 'claude') {
        // Non-Claude commands: use args as-is
        setResolvedArgs(args)
        setArgsReady(true)
        return
      }

      // Check for a saved editor session to resume
      const savedSession = await readEditorSession(cwd)
      if (cancelled) return

      if (savedSession) {
        // Resume previous session: strip any existing --resume + sessionId pair, then add ours
        const raw = args ?? []
        const resumeIdx = raw.indexOf('--resume')
        const baseArgs = resumeIdx === -1
          ? raw
          : [...raw.slice(0, resumeIdx), ...raw.slice(resumeIdx + 2)]
        setResolvedArgs([...baseArgs, '--resume', savedSession])
      } else {
        setResolvedArgs(args)
      }
      setArgsReady(true)
    }
    resolve()
    return () => { cancelled = true }
  }, [command, args, cwd])

  // ── File watching + polling fallback ─────────────────────────────────
  const lastContentRef = useRef<string>('')
  const fileExtension = filePath.split('.').pop() || 'json'

  useEffect(() => {
    let unlisten: (() => void) | null = null

    const findAndRead = async () => {
      try {
        if (trackFileRename) {
          // Scan directory for renamed files — used by theme editor etc.
          const entries = await invoke<{ name: string; path: string; isDirectory: boolean; modifiedAt: number }[]>(
            'fs_read_dir', { path: watchDir }
          )
          const matching = entries
            .filter((e) => !e.isDirectory && e.name.endsWith(`.${fileExtension}`) && !e.name.startsWith('.'))
            .sort((a, b) => b.modifiedAt - a.modifiedAt)

          const target = matching[0]
          if (!target) return

          // Only update activeFilePath if the original was renamed (no longer in directory)
          const originalStillExists = entries.some((e) => e.path === filePath)
          if (!originalStillExists && target.path !== filePath) {
            setActiveFilePath(target.path)
          }

          const result = await invoke<{ content: string; path: string; name: string }>('fs_read_file', { path: target.path })
          const content = result.content
          if (content !== lastContentRef.current) {
            lastContentRef.current = content
            onFileChange(content)
          }
        } else {
          // Direct file read — no directory scanning, no rename detection
          const result = await invoke<{ content: string; path: string; name: string }>('fs_read_file', { path: filePath })
          const content = result.content
          if (content !== lastContentRef.current) {
            lastContentRef.current = content
            onFileChange(content)
          }
        }
      } catch {
        // Directory might not exist yet or file mid-write — ignore
      }
    }

    invoke('fs_watch_dir', { path: watchDir }).catch((err) => {
      console.warn('[ai-editor] Failed to start watcher:', err)
    })

    listen<{ path: string; kind: string }>('fs://change', () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
      debounceRef.current = setTimeout(findAndRead, 300)
    }).then((fn) => { unlisten = fn })

    const pollInterval = setInterval(findAndRead, 2000)

    return () => {
      unlisten?.()
      clearInterval(pollInterval)
      if (debounceRef.current) clearTimeout(debounceRef.current)
      invoke('fs_unwatch_dir', { path: watchDir }).catch((e) => console.warn('[ai-editor]', e))
    }
  }, [watchDir, filePath, fileExtension, trackFileRename, onFileChange])

  // ── Terminal cleanup on unmount ────────────────────────────────────
  const cwdRef = useRef(cwd)
  cwdRef.current = cwd

  useEffect(() => {
    const id = terminalIdRef.current
    const t = setTimeout(() => setTerminalReady(true), 100)
    return () => {
      clearTimeout(t)
      // Save session before killing so it can be resumed next time
      saveEditorSession(cwdRef.current, commandRef.current).finally(() => {
        invoke('terminal_kill', { id }).catch(() => {})
      })
    }
  }, [])

  // Explicit close: save session, kill terminal, then notify parent
  const handleClose = useCallback(async () => {
    // Save the current Claude session ID so we can resume next time
    await saveEditorSession(cwd, commandRef.current)
    invoke('terminal_kill', { id: terminalIdRef.current }).catch(() => {})
    onClose()
  }, [cwd, onClose])

  const fileName = filePath.split('/').pop() || 'file'

  return (
    <div className="flex flex-col h-full">
      {/* ── Top bar ── */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
        <button
          onClick={handleClose}
          className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer no-drag"
        >
          &larr; Back
        </button>
        <span className="text-xs font-medium text-[var(--color-text-primary)]">{title}</span>
        <span className="text-[10px] font-mono text-[var(--color-text-muted)] ml-auto truncate max-w-[40%]">
          {activeFilePath}
        </span>
        {onManualRefresh && (
          <button
            onClick={onManualRefresh}
            className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag flex-shrink-0"
          >
            Refresh Preview
          </button>
        )}
      </div>

      {/* ── Warning banner ── */}
      <div className="flex items-center gap-2 px-4 py-2 bg-amber-500/8 border-b border-amber-500/20 flex-shrink-0">
        <span className="text-amber-400 text-sm flex-shrink-0">&#9888;</span>
        <span className="text-[11px] text-amber-300/80 leading-relaxed">{warningText}</span>
      </div>

      {/* ── Instructions (optional) ── */}
      {instructions && (
        <div className="px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
          <p className="text-[11px] text-[var(--color-text-muted)] leading-relaxed">{instructions}</p>
        </div>
      )}

      {/* ── Split view: terminal + preview ── */}
      <div className="flex flex-1 min-h-0">
        {/* Terminal (left) — wait for session resolution before mounting */}
        <div className="flex-1 min-w-0 border-r border-[var(--color-border)]">
          {argsReady ? (
            <AlacrittyTerminalView
              terminalId={terminalIdRef.current}
              cwd={cwd}
              command={command}
              args={resolvedArgs}
            />
          ) : (
            <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
              Checking for previous session...
            </div>
          )}
        </div>

        {/* Preview (right) */}
        <div className="flex-1 min-w-0 overflow-hidden">
          {preview}
        </div>
      </div>
    </div>
  )
}

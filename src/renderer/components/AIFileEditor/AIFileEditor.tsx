import { useEffect, useRef, useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { AlacrittyTerminalView } from '../Terminal/AlacrittyTerminalView'

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
}: AIFileEditorProps): React.JSX.Element {
  const terminalIdRef = useRef(`ai-editor-${crypto.randomUUID()}`)
  const [terminalReady, setTerminalReady] = useState(false)
  const [activeFilePath, setActiveFilePath] = useState(filePath)
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // ── File watching + polling fallback ─────────────────────────────────
  // Instead of tracking a single file, we scan the watch directory for the
  // most recently modified file matching the extension. This handles renames,
  // atomic writes, and file recreation by the AI agent.
  const lastContentRef = useRef<string>('')
  const fileExtension = filePath.split('.').pop() || 'json'

  useEffect(() => {
    let unlisten: (() => void) | null = null

    const findAndRead = async () => {
      try {
        // List all matching files in the directory, find most recently modified
        const entries = await invoke<{ name: string; path: string; isDirectory: boolean; modifiedAt: number }[]>(
          'fs_read_dir', { path: watchDir }
        )
        const matching = entries
          .filter((e) => !e.isDirectory && e.name.endsWith(`.${fileExtension}`) && !e.name.startsWith('.'))
          .sort((a, b) => b.modifiedAt - a.modifiedAt)

        const target = matching[0]
        if (!target) return

        // Update the active file path if it changed (rename detection)
        setActiveFilePath((prev) => {
          if (prev !== target.path) return target.path
          return prev
        })

        const result = await invoke<{ content: string; path: string; name: string }>('fs_read_file', { path: target.path })
        const content = result.content
        if (content !== lastContentRef.current) {
          lastContentRef.current = content
          onFileChange(content)
        }
      } catch {
        // Directory might not exist yet or file mid-write — ignore
      }
    }

    // Start watching the directory
    invoke('fs_watch_dir', { path: watchDir }).catch((err) => {
      console.warn('[ai-editor] Failed to start watcher:', err)
    })

    // On any change in the directory, scan for the latest file
    listen<{ path: string; kind: string }>('fs://change', () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
      debounceRef.current = setTimeout(findAndRead, 300)
    }).then((fn) => { unlisten = fn })

    // Polling fallback: scan every 2s in case the watcher misses events
    const pollInterval = setInterval(findAndRead, 2000)

    return () => {
      unlisten?.()
      clearInterval(pollInterval)
      if (debounceRef.current) clearTimeout(debounceRef.current)
      invoke('fs_unwatch_dir', { path: watchDir }).catch((e) => console.warn('[ai-editor]', e))
    }
  }, [watchDir, fileExtension, onFileChange])

  // ── Terminal cleanup on unmount ────────────────────────────────────
  useEffect(() => {
    const id = terminalIdRef.current
    const t = setTimeout(() => setTerminalReady(true), 100)
    return () => {
      clearTimeout(t)
      invoke('terminal_kill', { id }).catch(() => {})
    }
  }, [])

  // Explicit close: kill terminal first, then notify parent
  const handleClose = useCallback(() => {
    invoke('terminal_kill', { id: terminalIdRef.current }).catch(() => {})
    onClose()
  }, [onClose])

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
        {/* Terminal (left) */}
        <div className="flex-1 min-w-0 border-r border-[var(--color-border)]">
          <AlacrittyTerminalView
            terminalId={terminalIdRef.current}
            cwd={cwd}
            command={command}
            args={args}
          />
        </div>

        {/* Preview (right) */}
        <div className="flex-1 min-w-0 overflow-hidden">
          {preview}
        </div>
      </div>
    </div>
  )
}

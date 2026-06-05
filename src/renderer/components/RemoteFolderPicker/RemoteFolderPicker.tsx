import React, { useEffect, useState, useCallback } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'

// A lightweight, self-contained directory browser over the REMOTE daemon's
// filesystem. Deliberately does NOT reuse the full FileTree component (too
// coupled to its store / local-fs assumptions) — this is a purpose-built
// folder picker that only needs: list dirs, enter a dir, go up, and confirm
// the current directory.
//
// All fs reads go through the host-aware `daemonCliGet`, so the browser
// automatically targets whichever remote is active (connect-host store).
//
// Opened via the promise-based `useRemoteFolderPickerStore.open()` —
// resolves with the chosen path, or null on cancel.

interface DirEntry {
  name: string
  path: string
  isDirectory: boolean
}

interface FsInfo {
  home: string
  separator: string
  os: string
}

/** Compute the parent directory of `path` using the host separator.
 *  Returns null at the filesystem root (no further "up"). */
function parentDir(path: string, sep: string): string | null {
  if (!path || path === sep) return null
  // Strip a single trailing separator (but never reduce the root away).
  let p = path
  if (p.length > sep.length && p.endsWith(sep)) p = p.slice(0, -sep.length)
  const idx = p.lastIndexOf(sep)
  if (idx < 0) return null
  if (idx === 0) return sep // parent is the root ("/")
  return p.slice(0, idx)
}

export default function RemoteFolderPicker(): React.JSX.Element | null {
  const isOpen = useRemoteFolderPickerStore((s) => s.isOpen)
  const select = useRemoteFolderPickerStore((s) => s.select)
  const cancel = useRemoteFolderPickerStore((s) => s.cancel)

  const [sep, setSep] = useState('/')
  const [cwd, setCwd] = useState<string | null>(null)
  const [entries, setEntries] = useState<DirEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Load the directory listing for `dir` (directories only).
  const browse = useCallback(async (dir: string): Promise<void> => {
    setLoading(true)
    setError(null)
    try {
      const all = await daemonCliGet<DirEntry[]>('fs/read-dir', { path: dir })
      const dirs = all
        .filter((e) => e.isDirectory)
        .sort((a, b) => a.name.localeCompare(b.name))
      setEntries(dirs)
      setCwd(dir)
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      // Keep cwd unchanged so the user can retry / go elsewhere.
    } finally {
      setLoading(false)
    }
  }, [])

  // On open: fetch host fs/info, seed at home (fallback '/').
  useEffect(() => {
    if (!isOpen) return
    let cancelled = false
    setEntries([])
    setCwd(null)
    setError(null)
    setLoading(true)
    ;(async () => {
      let start = '/'
      let separator = '/'
      try {
        const info = await daemonCliGet<FsInfo>('fs/info')
        if (info.separator) separator = info.separator
        if (info.home) start = info.home
      } catch {
        // fall back to root with '/' separator
      }
      if (cancelled) return
      setSep(separator)
      await browse(start)
    })()
    return () => {
      cancelled = true
    }
  }, [isOpen, browse])

  // Close on Escape.
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') cancel()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, cancel])

  if (!isOpen) return null

  const parent = cwd ? parentDir(cwd, sep) : null

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={cancel}
    >
      <div
        className="w-[560px] max-h-[80vh] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-3 border-b border-[var(--color-border)]">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Open Folder on Host
          </h2>
          <div className="flex items-center gap-2 mt-2">
            <button
              className="px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)] disabled:opacity-30 disabled:cursor-default"
              onClick={() => parent && browse(parent)}
              disabled={!parent || loading}
              title="Up one level"
            >
              ↑ Up
            </button>
            <p className="flex-1 text-[10px] text-[var(--color-text-muted)] break-all font-mono">
              {cwd ?? '…'}
            </p>
          </div>
        </div>

        {/* Listing */}
        <div className="flex-1 overflow-y-auto min-h-[200px]">
          {error ? (
            <div className="px-5 py-4">
              <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
                <p className="text-[11px] text-red-400 whitespace-pre-wrap">{error}</p>
              </div>
            </div>
          ) : loading ? (
            <div className="px-5 py-6 text-center text-xs text-[var(--color-text-muted)]">
              Loading…
            </div>
          ) : entries.length === 0 ? (
            <div className="px-5 py-6 text-center text-xs text-[var(--color-text-muted)]">
              No sub-folders here
            </div>
          ) : (
            <div className="py-1">
              {entries.map((e) => (
                <button
                  key={e.path}
                  className="w-full flex items-center gap-2 px-5 py-1.5 text-left text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.05]"
                  onDoubleClick={() => browse(e.path)}
                  onClick={() => browse(e.path)}
                >
                  <svg className="w-3.5 h-3.5 shrink-0 text-[var(--color-text-muted)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" />
                  </svg>
                  <span className="truncate">{e.name}</span>
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Actions */}
        <div className="px-5 py-3 flex gap-2 justify-end border-t border-[var(--color-border)]">
          <button
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
            onClick={cancel}
          >
            Cancel
          </button>
          <button
            className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
            onClick={() => cwd && select(cwd)}
            disabled={!cwd || loading}
          >
            Select this folder
          </button>
        </div>
      </div>
    </div>
  )
}

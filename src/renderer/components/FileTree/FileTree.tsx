import { useState, useCallback, useMemo, useRef } from 'react'
import { trpc } from '@/lib/trpc'
import { showContextMenu } from '@/lib/context-menu'
import { useFileTreeStore } from '@/stores/filetree'
import { useTabsStore } from '@/stores/tabs'

// ── Types ────────────────────────────────────────────────────────────

interface FileEntry {
  name: string
  path: string
  isDirectory: boolean
  size: number
  modifiedAt: number
}

interface FileTreeProps {
  rootPath: string
  onNavigate?: (path: string) => void
}

// ── Helpers ──────────────────────────────────────────────────────────

function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

// ── Icons ────────────────────────────────────────────────────────────

function ChevronIcon({ expanded }: { expanded: boolean }): React.JSX.Element {
  return (
    <svg
      className={`w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)] transition-transform duration-100 ${
        expanded ? 'rotate-90' : ''
      }`}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M9 5l7 7-7 7" />
    </svg>
  )
}

function FolderIcon({ open }: { open: boolean }): React.JSX.Element {
  if (open) {
    return (
      <svg
        className="w-4 h-4 flex-shrink-0"
        viewBox="0 0 24 24"
        fill="#EAB308"
        stroke="#CA8A04"
        strokeWidth={1}
      >
        <path d="M5 4h4l2 2h8a1 1 0 011 1v1H4V5a1 1 0 011-1z" />
        <path d="M3.5 9h17l-1.5 11H5L3.5 9z" />
      </svg>
    )
  }
  return (
    <svg
      className="w-4 h-4 flex-shrink-0"
      viewBox="0 0 24 24"
      fill="#EAB308"
      stroke="#CA8A04"
      strokeWidth={1}
    >
      <path d="M5 4h4l2 2h8a1 1 0 011 1v12a1 1 0 01-1 1H5a1 1 0 01-1-1V5a1 1 0 011-1z" />
    </svg>
  )
}

function FileIcon(): React.JSX.Element {
  return (
    <svg
      className="w-4 h-4 flex-shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="#9CA3AF"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z" />
      <path d="M14 2v6h6" />
    </svg>
  )
}

// ── TreeItem ─────────────────────────────────────────────────────────

function TreeItem({
  entry,
  depth,
  cache,
  expandedDirs,
  loadingDirs,
  errorDirs,
  onToggleDir,
  onClickFile,
  onContextMenu,
  searchQuery
}: {
  entry: FileEntry
  depth: number
  cache: Map<string, FileEntry[]>
  expandedDirs: Set<string>
  loadingDirs: Set<string>
  errorDirs: Map<string, string>
  onToggleDir: (path: string) => void
  onClickFile: (entry: FileEntry) => void
  onContextMenu: (e: React.MouseEvent, entry: FileEntry) => void
  searchQuery: string
}): React.JSX.Element | null {
  const isExpanded = expandedDirs.has(entry.path)
  const isLoading = loadingDirs.has(entry.path)
  const error = errorDirs.get(entry.path)
  const children = cache.get(entry.path)

  const filteredChildren = useMemo(() => {
    if (!children) return null
    if (!searchQuery) return children
    return children.filter((child) => {
      // Show directories that have matching descendants or match themselves
      if (child.isDirectory) {
        if (child.name.toLowerCase().includes(searchQuery.toLowerCase())) return true
        // If expanded, let recursive rendering handle filtering
        if (expandedDirs.has(child.path)) return true
        return false
      }
      return child.name.toLowerCase().includes(searchQuery.toLowerCase())
    })
  }, [children, searchQuery, expandedDirs])

  const handleClick = useCallback(() => {
    if (entry.isDirectory) {
      onToggleDir(entry.path)
    } else {
      onClickFile(entry)
    }
  }, [entry, onToggleDir, onClickFile])

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      onContextMenu(e, entry)
    },
    [entry, onContextMenu]
  )

  return (
    <div>
      <button
        className="w-full flex items-center gap-1 py-[3px] text-left text-[13px] leading-tight transition-colors  hover:bg-white/[0.06] group"
        style={{ paddingLeft: depth * 16 + 8 }}
        onClick={handleClick}
        onContextMenu={handleContextMenu}
      >
        {/* Chevron (dirs only) */}
        <span className="w-3 flex-shrink-0 flex items-center justify-center">
          {entry.isDirectory ? <ChevronIcon expanded={isExpanded} /> : null}
        </span>

        {/* Icon */}
        {entry.isDirectory ? <FolderIcon open={isExpanded} /> : <FileIcon />}

        {/* Name */}
        <span className="truncate text-[var(--color-text-secondary)] group-hover:text-[var(--color-text-primary)]">
          {entry.name}
        </span>

        {/* File size (files only) */}
        {!entry.isDirectory && entry.size > 0 && (
          <span className="ml-auto pr-2 text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0 opacity-60">
            {formatFileSize(entry.size)}
          </span>
        )}
      </button>

      {/* Children (expanded dirs) */}
      {entry.isDirectory && isExpanded && (
        <div>
          {isLoading && (
            <div
              className="py-1 text-[11px] text-[var(--color-text-muted)] italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              Loading...
            </div>
          )}
          {error && (
            <div
              className="py-1 text-[11px] text-red-400 italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              {error}
            </div>
          )}
          {filteredChildren &&
            filteredChildren.map((child) => (
              <TreeItem
                key={child.path}
                entry={child}
                depth={depth + 1}
                cache={cache}
                expandedDirs={expandedDirs}
                loadingDirs={loadingDirs}
                errorDirs={errorDirs}
                onToggleDir={onToggleDir}
                onClickFile={onClickFile}
                onContextMenu={onContextMenu}
                searchQuery={searchQuery}
              />
            ))}
          {filteredChildren && filteredChildren.length === 0 && !isLoading && !error && (
            <div
              className="py-1 text-[11px] text-[var(--color-text-muted)] italic"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              {searchQuery ? 'No matches' : 'Empty'}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ── FileTree ─────────────────────────────────────────────────────────

export default function FileTree({ rootPath, onNavigate }: FileTreeProps): React.JSX.Element {
  const searchQuery = useFileTreeStore((s) => s.searchQuery)
  const setSearchQuery = useFileTreeStore((s) => s.setSearchQuery)

  // Cache: path -> entries
  const [cache, setCache] = useState<Map<string, FileEntry[]>>(new Map())
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set([rootPath]))
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set())
  const [errorDirs, setErrorDirs] = useState<Map<string, string>>(new Map())

  // Track the root path so we can reset when it changes
  const prevRootPath = useRef(rootPath)
  if (prevRootPath.current !== rootPath) {
    prevRootPath.current = rootPath
    setCache(new Map())
    setExpandedDirs(new Set([rootPath]))
    setLoadingDirs(new Set())
    setErrorDirs(new Map())
  }

  // Load directory contents
  const loadDir = useCallback(
    async (dirPath: string) => {
      if (cache.has(dirPath)) return

      setLoadingDirs((prev) => new Set(prev).add(dirPath))
      setErrorDirs((prev) => {
        const next = new Map(prev)
        next.delete(dirPath)
        return next
      })

      try {
        const entries = await trpc.fs.readDir.query({ path: dirPath })
        setCache((prev) => new Map(prev).set(dirPath, entries))
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to read directory'
        setErrorDirs((prev) => new Map(prev).set(dirPath, message))
      } finally {
        setLoadingDirs((prev) => {
          const next = new Set(prev)
          next.delete(dirPath)
          return next
        })
      }
    },
    [cache]
  )

  // Toggle directory expand/collapse
  const handleToggleDir = useCallback(
    (dirPath: string) => {
      setExpandedDirs((prev) => {
        const next = new Set(prev)
        if (next.has(dirPath)) {
          next.delete(dirPath)
        } else {
          next.add(dirPath)
          // Load contents if not cached
          loadDir(dirPath)
        }
        return next
      })
    },
    [loadDir]
  )

  // File click -> open file in a new tab
  const handleClickFile = useCallback(
    (entry: FileEntry) => {
      useTabsStore.getState().openFileInNewTab(entry.path)
    },
    []
  )

  // Context menu
  const handleContextMenu = useCallback(async (e: React.MouseEvent, entry: FileEntry) => {
    e.preventDefault()

    const clickedId = await showContextMenu([
      { id: 'open-finder', label: 'Open in Finder' },
      { id: 'copy-path', label: 'Copy Path' }
    ])

    if (clickedId === 'open-finder') {
      await trpc.fs.openInFinder.mutate({ path: entry.path })
    } else if (clickedId === 'copy-path') {
      await trpc.fs.copyPath.mutate({ path: entry.path })
    }
  }, [])

  // Load root on first expand
  const rootEntries = cache.get(rootPath)
  if (!rootEntries && !loadingDirs.has(rootPath) && !errorDirs.has(rootPath)) {
    loadDir(rootPath)
  }

  // Filter root entries by search
  const filteredRootEntries = useMemo(() => {
    if (!rootEntries) return null
    if (!searchQuery) return rootEntries
    return rootEntries.filter((entry) => {
      if (entry.isDirectory) {
        if (entry.name.toLowerCase().includes(searchQuery.toLowerCase())) return true
        if (expandedDirs.has(entry.path)) return true
        return false
      }
      return entry.name.toLowerCase().includes(searchQuery.toLowerCase())
    })
  }, [rootEntries, searchQuery, expandedDirs])

  const rootBasename = rootPath.split('/').pop() || rootPath

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-3 pt-3 pb-2">
        <span className="text-xs font-semibold tracking-widest text-[var(--color-text-muted)] uppercase">
          Files
        </span>
      </div>

      {/* Search input */}
      <div className="px-3 pb-2">
        <div className="relative">
          <svg
            className="absolute left-2 top-1/2 -translate-y-1/2 w-3 h-3 text-[var(--color-text-muted)]"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={2}
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Filter files..."
            className="w-full pl-7 pr-2 py-1.5 text-xs bg-white/[0.04] border border-[var(--color-border)]  text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] transition-colors"
          />
          {searchQuery && (
            <button
              className="absolute right-1.5 top-1/2 -translate-y-1/2 w-4 h-4 flex items-center justify-centertext-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              onClick={() => setSearchQuery('')}
            >
              <svg
                className="w-3 h-3"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2}
              >
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {/* Tree */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden px-1 pb-2">
        {/* Root folder header */}
        <button
          className="w-full flex items-center gap-1 py-[3px] px-2 text-left text-[13px] font-medium text-[var(--color-text-primary)] hover:bg-white/[0.06] "
          onClick={() => handleToggleDir(rootPath)}
          onContextMenu={(e) =>
            handleContextMenu(e, {
              name: rootBasename,
              path: rootPath,
              isDirectory: true,
              size: 0,
              modifiedAt: 0
            })
          }
        >
          <span className="w-3 flex-shrink-0 flex items-center justify-center">
            <ChevronIcon expanded={expandedDirs.has(rootPath)} />
          </span>
          <FolderIcon open={expandedDirs.has(rootPath)} />
          <span className="truncate">{rootBasename}</span>
        </button>

        {/* Root contents */}
        {expandedDirs.has(rootPath) && (
          <div>
            {loadingDirs.has(rootPath) && (
              <div className="py-1 pl-10 text-[11px] text-[var(--color-text-muted)] italic">
                Loading...
              </div>
            )}
            {errorDirs.has(rootPath) && (
              <div className="py-1 pl-10 text-[11px] text-red-400 italic">
                {errorDirs.get(rootPath)}
              </div>
            )}
            {filteredRootEntries &&
              filteredRootEntries.map((entry) => (
                <TreeItem
                  key={entry.path}
                  entry={entry}
                  depth={1}
                  cache={cache}
                  expandedDirs={expandedDirs}
                  loadingDirs={loadingDirs}
                  errorDirs={errorDirs}
                  onToggleDir={handleToggleDir}
                  onClickFile={handleClickFile}
                  onContextMenu={handleContextMenu}
                  searchQuery={searchQuery}
                />
              ))}
            {filteredRootEntries && filteredRootEntries.length === 0 && (
              <div className="py-1 pl-10 text-[11px] text-[var(--color-text-muted)] italic">
                {searchQuery ? 'No matches' : 'Empty'}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

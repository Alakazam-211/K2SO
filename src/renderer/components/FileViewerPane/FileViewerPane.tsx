import { useState, useEffect, useCallback, useRef } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { invoke, convertFileSrc } from '@tauri-apps/api/core'
import { PDFViewer } from './PDFViewer'
import { DocxViewer } from './DocxViewer'
import { HighlightedCodeBlock } from './CodeHighlighter'
import { CodeEditor } from './CodeEditor'
import { useTabsStore } from '@/stores/tabs'
import { FILE_POLL_INTERVAL } from '@shared/constants'

// ── Types ────────────────────────────────────────────────────────────────

interface FileViewerPaneProps {
  filePath: string
  paneId: string
  tabId: string
  onClose?: () => void
}

type FileCategory = 'markdown' | 'image' | 'pdf' | 'docx' | 'text'
type ViewMode = 'rendered' | 'raw'

// ── Helpers ──────────────────────────────────────────────────────────────

const MARKDOWN_EXTS = ['.md', '.markdown', '.mdx']
const IMAGE_EXTS = ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.bmp', '.ico']
const PDF_EXTS = ['.pdf']
const DOCX_EXTS = ['.docx', '.doc']

function getFileCategory(filePath: string): FileCategory {
  const ext = filePath.toLowerCase().replace(/^.*(\.[^.]+)$/, '$1')
  if (MARKDOWN_EXTS.includes(ext)) return 'markdown'
  if (IMAGE_EXTS.includes(ext)) return 'image'
  if (PDF_EXTS.includes(ext)) return 'pdf'
  if (DOCX_EXTS.includes(ext)) return 'docx'
  return 'text'
}

function getDefaultViewMode(category: FileCategory): ViewMode {
  if (category === 'markdown' || category === 'image') return 'rendered'
  return 'raw'
}

function getFileName(filePath: string): string {
  return filePath.split('/').pop() || filePath
}

function getShortPath(filePath: string): string {
  if (filePath.length > 60) return '...' + filePath.slice(-57)
  return filePath
}

// ── Component ────────────────────────────────────────────────────────────

export function FileViewerPane({ filePath, paneId, tabId, onClose }: FileViewerPaneProps): React.JSX.Element {
  const [content, setContent] = useState<string>('')
  const [editedContent, setEditedContent] = useState<string | null>(null) // null = not edited
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const category = getFileCategory(filePath)
  const [viewMode, setViewMode] = useState<ViewMode>(getDefaultViewMode(category))
  const isDirty = editedContent !== null && editedContent !== content
  const setTabDirty = useTabsStore((s) => s.setTabDirty)

  const pinned = useTabsStore((s) => {
    const tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) return false
    // Search all paneGroups for an item matching paneId
    for (const [, pg] of tab.paneGroups) {
      for (const item of pg.items) {
        if (item.id === paneId && item.type === 'file-viewer') {
          return item.pinned ?? false
        }
      }
    }
    return false
  })
  const pinPane = useTabsStore((s) => s.pinPane)
  const unpinPane = useTabsStore((s) => s.unpinPane)

  const [searchQuery, setSearchQuery] = useState('')
  const [searchVisible, setSearchVisible] = useState(false)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)

  const fileName = getFileName(filePath)
  const shortPath = getShortPath(filePath)

  // Reset view mode when file changes
  useEffect(() => {
    const newCategory = getFileCategory(filePath)
    setViewMode(getDefaultViewMode(newCategory))
  }, [filePath])

  const loadFile = useCallback(async () => {
    // Images, PDFs, and DOCX files don't need text content
    if (getFileCategory(filePath) === 'image' || getFileCategory(filePath) === 'pdf' || getFileCategory(filePath) === 'docx') {
      setLoading(false)
      setError(null)
      return
    }

    setLoading(true)
    setError(null)
    try {
      const result = await invoke<{ content: string }>('fs_read_file', { path: filePath })
      setContent(result.content)
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to read file'
      setError(message)
    } finally {
      setLoading(false)
    }
  }, [filePath])

  useEffect(() => {
    loadFile()
  }, [loadFile])

  // Sync dirty state to tab
  useEffect(() => {
    setTabDirty(tabId, isDirty)
  }, [isDirty, tabId, setTabDirty])

  // Auto-refresh: poll for file changes every 2 seconds (only when not editing)
  useEffect(() => {
    if (getFileCategory(filePath) === 'image' || getFileCategory(filePath) === 'pdf' || getFileCategory(filePath) === 'docx') return
    if (isDirty) return // Don't overwrite user edits

    const interval = setInterval(async () => {
      try {
        const result = await invoke<{ content: string }>('fs_read_file', { path: filePath })
        if (result.content !== content) {
          setContent(result.content)
        }
      } catch {
        // Ignore polling errors
      }
    }, FILE_POLL_INTERVAL)

    return () => clearInterval(interval)
  }, [filePath, content, isDirty])

  // Save file (Cmd+S) — called directly by CodeEditor with current content
  const saveFile = useCallback(async (contentToSave?: string) => {
    const toSave = contentToSave ?? editedContent
    if (toSave === null || toSave === undefined) return
    if (toSave === content) return // Nothing changed
    setSaving(true)
    try {
      await invoke('fs_write_file', { path: filePath, content: toSave })
      setContent(toSave)
      setEditedContent(null)
    } catch (err) {
      console.error('[file-viewer] Save failed:', err)
    } finally {
      setSaving(false)
    }
  }, [filePath, editedContent, content])

  // Cmd+F search and Cmd+S save shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'f') {
        e.preventDefault()
        e.stopPropagation()
        setSearchVisible(true)
        requestAnimationFrame(() => {
          searchInputRef.current?.focus()
          searchInputRef.current?.select()
        })
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault()
        e.stopPropagation()
        saveFile(editedContent ?? undefined)
      }
      if (e.key === 'Escape' && searchVisible) {
        setSearchVisible(false)
        setSearchQuery('')
      }
    }

    const container = contentRef.current?.parentElement
    if (container) {
      container.addEventListener('keydown', handleKeyDown)
      return () => container.removeEventListener('keydown', handleKeyDown)
    }
  }, [searchVisible, saveFile, editedContent])

  // Highlight search matches
  useEffect(() => {
    if (!contentRef.current || !searchQuery) return

    const existing = contentRef.current.querySelectorAll('mark[data-fv-search]')
    existing.forEach((el) => {
      const parent = el.parentNode
      if (parent) {
        parent.replaceChild(document.createTextNode(el.textContent || ''), el)
        parent.normalize()
      }
    })

    if (!searchQuery.trim()) return

    const walker = document.createTreeWalker(contentRef.current, NodeFilter.SHOW_TEXT)
    const matches: { node: Text; index: number }[] = []
    const query = searchQuery.toLowerCase()

    let node: Text | null
    while ((node = walker.nextNode() as Text | null)) {
      const text = node.textContent?.toLowerCase() || ''
      let idx = text.indexOf(query)
      while (idx !== -1) {
        matches.push({ node, index: idx })
        idx = text.indexOf(query, idx + 1)
      }
    }

    for (let i = matches.length - 1; i >= 0; i--) {
      const { node: textNode, index } = matches[i]
      const range = document.createRange()
      range.setStart(textNode, index)
      range.setEnd(textNode, index + searchQuery.length)

      const mark = document.createElement('mark')
      mark.setAttribute('data-fv-search', '')
      mark.style.background = '#b5890066'
      mark.style.color = 'inherit'
      mark.style.borderRadius = '0'
      range.surroundContents(mark)
    }

    const firstMark = contentRef.current.querySelector('mark[data-fv-search]')
    if (firstMark) {
      firstMark.scrollIntoView({ block: 'center', behavior: 'smooth' })
    }
  }, [searchQuery, content, viewMode])

  const handleTogglePin = useCallback(() => {
    if (pinned) {
      unpinPane(tabId, paneId)
    } else {
      pinPane(tabId, paneId)
    }
  }, [pinned, tabId, paneId, pinPane, unpinPane])

  // Show toggle only for markdown and image files (not PDF)
  const showViewToggle = category === 'markdown' || category === 'image'

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-sm">
        Loading...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col bg-[#0a0a0a]">
        {/* Toolbar even in error state */}
        <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
          <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
            {fileName}
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)] truncate hidden sm:inline" title={filePath}>
            {shortPath}
          </span>
          <div className="flex-1" />
          {onClose && (
            <button
              className="p-1 text-[var(--color-text-muted)] hover:text-red-400 transition-colors"
              onClick={onClose}
              title="Close"
            >
              <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
        <div className="flex flex-1 flex-col items-center justify-center gap-3">
          <span className="text-red-400 text-sm">{error}</span>
          <button
            className="text-xs text-[var(--color-accent)] hover:underline"
            onClick={loadFile}
          >
            Retry
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full w-full flex-col bg-[#0a0a0a] no-drag" tabIndex={-1}>
      {/* Toolbar */}
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
        {/* File info */}
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {fileName}
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] truncate hidden sm:inline" title={filePath}>
          {shortPath}
        </span>

        {/* Dirty / saving indicator */}
        {isDirty && (
          <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0">
            {saving ? 'Saving...' : 'Modified'}
          </span>
        )}

        <div className="flex-1" />

        {/* Search toggle */}
        <button
          className={`p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors ${searchVisible ? 'text-[var(--color-accent)]' : ''}`}
          onClick={() => {
            setSearchVisible(!searchVisible)
            if (!searchVisible) {
              requestAnimationFrame(() => searchInputRef.current?.focus())
            } else {
              setSearchQuery('')
            }
          }}
          title="Search (Cmd+F)"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
        </button>

        {/* View mode toggle (markdown and images only) */}
        {showViewToggle && (
          <div className="flex border border-[var(--color-border)]">
            <button
              className={`px-2 py-0.5 text-[10px] font-medium transition-colors ${
                viewMode === 'rendered'
                  ? 'bg-[var(--color-accent)] text-white'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
              onClick={() => setViewMode('rendered')}
            >
              Rendered
            </button>
            <button
              className={`px-2 py-0.5 text-[10px] font-medium transition-colors ${
                viewMode === 'raw'
                  ? 'bg-[var(--color-accent)] text-white'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
              onClick={() => setViewMode('raw')}
            >
              Raw
            </button>
          </div>
        )}

        {/* Refresh */}
        <button
          className="p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
          onClick={loadFile}
          title="Refresh"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <path d="M21.5 2v6h-6" />
            <path d="M2.5 22v-6h6" />
            <path d="M22 11.5A10 10 0 003.2 7.2" />
            <path d="M2 12.5a10 10 0 0018.8 4.3" />
          </svg>
        </button>

        {/* Pin/Unpin */}
        <button
          className={`p-1 transition-colors ${
            pinned
              ? 'text-[var(--color-accent)]'
              : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
          }`}
          onClick={handleTogglePin}
          title={pinned ? 'Unpin (preview mode)' : 'Pin (keep open)'}
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill={pinned ? 'currentColor' : 'none'} stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <path d="M12 2l0 5" />
            <path d="M6 7h12l-1.5 8H7.5L6 7z" />
            <path d="M9 15l-2 7" />
            <path d="M15 15l2 7" />
          </svg>
        </button>

        {/* Close */}
        {onClose && (
          <button
            className="p-1 text-[var(--color-text-muted)] hover:text-red-400 transition-colors"
            onClick={onClose}
            title="Close"
          >
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        )}
      </div>

      {/* Search bar */}
      {searchVisible && (
        <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1.5 flex-shrink-0">
          <svg className="w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            ref={searchInputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search in document..."
            className="flex-1 bg-transparent text-xs text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
          />
          {searchQuery && (
            <button
              className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              onClick={() => setSearchQuery('')}
            >
              <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          )}
        </div>
      )}

      {/* Content */}
      {category === 'pdf' ? (
        <div className="flex-1 overflow-hidden">
          <PDFViewer filePath={filePath} />
        </div>
      ) : category === 'docx' ? (
        <div className="flex-1 overflow-hidden">
          <DocxViewer filePath={filePath} />
        </div>
      ) : category === 'image' && viewMode === 'rendered' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="flex items-center justify-center p-4 min-h-full bg-[#0a0a0a]">
            <img
              src={convertFileSrc(filePath)}
              alt={fileName}
              style={{ maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' }}
              onError={(e) => {
                (e.target as HTMLImageElement).style.display = 'none'
                setError('Failed to load image')
              }}
            />
          </div>
        </div>
      ) : category === 'image' && viewMode === 'raw' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="p-4 text-xs text-[var(--color-text-muted)]">
            <p>Binary image file. Switch to Rendered mode to view.</p>
          </div>
        </div>
      ) : category === 'markdown' && viewMode === 'rendered' ? (
        <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
          <div className="markdown-content p-4">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                code({ className, children, ...props }) {
                  const match = /language-(\w+)/.exec(className || '')
                  const codeStr = String(children).replace(/\n$/, '')
                  if (match) {
                    return <HighlightedCodeBlock code={codeStr} language={match[1]} />
                  }
                  return <code className={className} {...props}>{children}</code>
                }
              }}
            >
              {content}
            </ReactMarkdown>
          </div>
        </div>
      ) : (
        <div className="flex-1 overflow-hidden">
          <CodeEditor
            code={editedContent ?? content}
            filePath={filePath}
            onSave={saveFile}
            onChange={(newContent) => setEditedContent(newContent)}
          />
        </div>
      )}
    </div>
  )
}

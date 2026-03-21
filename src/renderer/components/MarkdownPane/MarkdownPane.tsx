import { useState, useEffect, useCallback, useRef } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { trpc } from '@/lib/trpc'

interface MarkdownPaneProps {
  filePath: string
  onClose?: () => void
}

type ViewMode = 'rendered' | 'raw'

export function MarkdownPane({ filePath, onClose }: MarkdownPaneProps): React.JSX.Element {
  const [content, setContent] = useState<string>('')
  const [fileName, setFileName] = useState<string>('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [viewMode, setViewMode] = useState<ViewMode>('rendered')
  const [searchQuery, setSearchQuery] = useState('')
  const [searchVisible, setSearchVisible] = useState(false)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)

  const loadFile = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const result = await trpc.fs.readFile.query({ path: filePath })
      setContent(result.content)
      setFileName(result.name)
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

  // Cmd+F search shortcut
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
  }, [searchVisible])

  // Highlight search matches using CSS highlight API or manual approach
  useEffect(() => {
    if (!contentRef.current || !searchQuery) return

    // Clear previous highlights
    const existing = contentRef.current.querySelectorAll('mark[data-md-search]')
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

    // Apply highlights (in reverse to preserve indices)
    for (let i = matches.length - 1; i >= 0; i--) {
      const { node: textNode, index } = matches[i]
      const range = document.createRange()
      range.setStart(textNode, index)
      range.setEnd(textNode, index + searchQuery.length)

      const mark = document.createElement('mark')
      mark.setAttribute('data-md-search', '')
      mark.style.background = '#b5890066'
      mark.style.color = 'inherit'
      mark.style.borderRadius = '0'
      range.surroundContents(mark)
    }

    // Scroll first match into view
    const firstMark = contentRef.current.querySelector('mark[data-md-search]')
    if (firstMark) {
      firstMark.scrollIntoView({ block: 'center', behavior: 'smooth' })
    }
  }, [searchQuery, content, viewMode])

  const shortPath = filePath.length > 60 ? '...' + filePath.slice(-57) : filePath

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-sm">
        Loading...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center bg-[#0a0a0a] gap-3">
        <span className="text-red-400 text-sm">{error}</span>
        <button
          className="text-xs text-[var(--color-accent)] hover:underline"
          onClick={loadFile}
        >
          Retry
        </button>
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

        {/* View mode toggle */}
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
      <div className="flex-1 overflow-y-auto overflow-x-hidden" ref={contentRef}>
        {viewMode === 'rendered' ? (
          <div className="markdown-content p-4">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
          </div>
        ) : (
          <pre className="p-4 text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap break-words">
            <code>{content}</code>
          </pre>
        )}
      </div>
    </div>
  )
}

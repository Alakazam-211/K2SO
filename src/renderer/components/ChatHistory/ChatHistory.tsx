import { useState, useEffect, useMemo, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'

// ── Types ────────────────────────────────────────────────────────────

interface ChatSession {
  sessionId: string
  project: string
  title: string
  timestamp: number // unix ms
  provider: string
  messageCount: number
}

type DateGroup = 'Today' | 'Yesterday' | 'This Week' | 'This Month' | 'Older'

// ── CLI tool config ─────────────────────────────────────────────────

const PROVIDER_CONFIG: Record<string, { command: string; label: string }> = {
  claude: { command: 'claude', label: 'Claude' },
  cursor: { command: 'cursor-agent', label: 'Cursor' },
}

// ── Helpers ──────────────────────────────────────────────────────────

function classifyDate(timestamp: number): DateGroup {
  const now = new Date()
  const date = new Date(timestamp)

  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const startOfYesterday = new Date(startOfToday.getTime() - 86400000)
  const startOfWeek = new Date(startOfToday)
  startOfWeek.setDate(startOfToday.getDate() - startOfToday.getDay())
  const startOfMonth = new Date(now.getFullYear(), now.getMonth(), 1)

  if (date >= startOfToday) return 'Today'
  if (date >= startOfYesterday) return 'Yesterday'
  if (date >= startOfWeek) return 'This Week'
  if (date >= startOfMonth) return 'This Month'
  return 'Older'
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp)
  const group = classifyDate(timestamp)

  if (group === 'Today' || group === 'Yesterday') {
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  }

  return date.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

const GROUP_ORDER: DateGroup[] = ['Today', 'Yesterday', 'This Week', 'This Month', 'Older']

/** Get the right-most leaf node ID in a mosaic tree */
function getRightmostLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'second' in tree) {
    return getRightmostLeaf((tree as { second: unknown }).second)
  }
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getRightmostLeaf((tree as { first: unknown }).first)
  }
  return null
}

// ── Icons ────────────────────────────────────────────────────────────

function ClaudeIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3.5 h-3.5 flex-shrink-0 text-[var(--color-text-muted)]"
      viewBox="0 0 16 16"
      fill="none"
    >
      <path
        d="M8 2l1.5 3.5L13 7l-3.5 1.5L8 12l-1.5-3.5L3 7l3.5-1.5z"
        fill="currentColor"
      />
    </svg>
  )
}

function CursorIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3.5 h-3.5 flex-shrink-0 text-[var(--color-text-muted)]"
      viewBox="0 0 16 16"
      fill="none"
    >
      <path
        d="M3 2l10 6-10 6V2z"
        fill="currentColor"
        opacity="0.8"
      />
    </svg>
  )
}

function ProviderIcon({ provider }: { provider: string }): React.JSX.Element {
  if (provider === 'cursor') return <CursorIcon />
  return <ClaudeIcon />
}

interface ChatStoragePaths {
  claudeHistoryFile: string | null
  claudeSessionsDirs: string[]
  cursorChatsDirs: string[]
}

function SearchIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="11" cy="11" r="8" />
      <path d="M21 21l-4.35-4.35" />
    </svg>
  )
}

function RefreshIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M21 2v6h-6" />
      <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
      <path d="M3 22v-6h6" />
      <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
    </svg>
  )
}

// ── Component ────────────────────────────────────────────────────────

export default function ChatHistory(): React.JSX.Element {
  const [sessions, setSessions] = useState<ChatSession[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [searchVisible, setSearchVisible] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(-1)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const selectedRowRef = useRef<HTMLButtonElement>(null)
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const projectPath = activeWorkspace?.worktreePath ?? activeProject?.path

  const fetchSessions = useCallback(async (showLoading = false) => {
    if (!projectPath) {
      setSessions([])
      setLoading(false)
      return
    }

    if (showLoading) setLoading(true)
    setError(null)

    try {
      const result = await invoke<ChatSession[]>('chat_history_list_for_project', { projectPath })
      setSessions(result)
    } catch (e) {
      console.error('[chat-history]', e)
      setError(String(e))
      setSessions([])
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  // Initial fetch
  useEffect(() => {
    fetchSessions(true)
  }, [fetchSessions])

  // Poll every 5 seconds to catch new sessions quickly
  // (cheap operation — reads small files)
  useEffect(() => {
    pollIntervalRef.current = setInterval(() => {
      fetchSessions(false) // silent refresh, no loading indicator
    }, 5000)

    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current)
      }
    }
  }, [fetchSessions])

  const grouped = useMemo(() => {
    const groups = new Map<DateGroup, ChatSession[]>()

    // Filter by search query if present
    const q = searchQuery.toLowerCase().trim()
    const filtered = q
      ? sessions.filter((s) => s.title.toLowerCase().includes(q))
      : sessions

    // Sort newest first
    const sorted = [...filtered].sort((a, b) => b.timestamp - a.timestamp)

    for (const session of sorted) {
      const group = classifyDate(session.timestamp)
      const existing = groups.get(group)
      if (existing) {
        existing.push(session)
      } else {
        groups.set(group, [session])
      }
    }

    return groups
  }, [sessions, searchQuery])

  // Flat ordered list of visible sessions for keyboard navigation
  const flatSessions = useMemo(() => {
    const result: ChatSession[] = []
    for (const group of GROUP_ORDER) {
      const items = grouped.get(group)
      if (items) result.push(...items)
    }
    return result
  }, [grouped])

  // Reset selection when search query changes
  useEffect(() => {
    setSelectedIndex(-1)
  }, [searchQuery])

  // Scroll selected row into view
  useEffect(() => {
    selectedRowRef.current?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  const handleAgenticSearch = useCallback(async () => {
    if (!projectPath || !searchQuery.trim()) return

    const paths = await invoke<ChatStoragePaths>('chat_history_get_storage_paths', { projectPath })
    const agent = useSettingsStore.getState().defaultAgent || 'claude'

    // Build the search prompt with available paths
    const locationLines: string[] = []
    if (paths.claudeHistoryFile) locationLines.push(`- Claude session index: ${paths.claudeHistoryFile}`)
    for (const dir of paths.claudeSessionsDirs) locationLines.push(`- Claude sessions: ${dir}`)
    for (const dir of paths.cursorChatsDirs) locationLines.push(`- Cursor chats: ${dir}`)

    const prompt = [
      `Search through my conversation history for: "${searchQuery.trim()}"`,
      '',
      locationLines.length > 0
        ? `History locations:\n${locationLines.join('\n')}`
        : 'No conversation history files were found for this project.',
      '',
      'Read the relevant files and show which conversations match, with titles, dates, and relevant excerpts.',
    ].join('\n')

    const tabsStore = useTabsStore.getState()
    const targetGroup = tabsStore.splitCount > 1 ? tabsStore.splitCount - 1 : 0

    // Claude uses -p for print mode; other agents get the prompt as a positional arg
    const args = agent === 'claude' ? ['-p', prompt] : [prompt]

    tabsStore.addTabToGroup(targetGroup, projectPath, {
      title: `Search: ${searchQuery.trim().slice(0, 30)}`,
      command: agent,
      args,
    })
  }, [projectPath, searchQuery])

  const handleSessionClick = useCallback(
    (session: ChatSession) => {
      if (!projectPath) return

      const config = PROVIDER_CONFIG[session.provider]
      if (!config) return

      const tabsStore = useTabsStore.getState()

      // If split into columns, open in the rightmost group
      const targetGroup = tabsStore.splitCount > 1 ? tabsStore.splitCount - 1 : 0

      tabsStore.addTabToGroup(targetGroup, projectPath, {
        title: session.title,
        command: config.command,
        args: ['--resume', session.sessionId]
      })
    },
    [projectPath]
  )

  const handleSearchKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        setSearchQuery('')
        setSearchVisible(false)
        setSelectedIndex(-1)
      } else if (e.key === 'Enter' && e.metaKey) {
        e.preventDefault()
        handleAgenticSearch()
      } else if (e.key === 'Enter' && !e.metaKey && selectedIndex >= 0 && selectedIndex < flatSessions.length) {
        e.preventDefault()
        handleSessionClick(flatSessions[selectedIndex])
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((i) => Math.min(i + 1, flatSessions.length - 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((i) => Math.max(i - 1, -1))
      }
    },
    [handleAgenticSearch, handleSessionClick, flatSessions, selectedIndex]
  )

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)] font-mono">No project selected</p>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex items-center justify-between flex-shrink-0">
        <span className="text-xs font-medium text-[var(--color-text-secondary)] font-mono">
          Chat History
        </span>
        <div className="flex items-center gap-0.5">
          <button
            className={`no-drag p-1 transition-colors ${
              searchVisible
                ? 'text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
            onClick={() => {
              setSearchVisible((v) => !v)
              if (!searchVisible) {
                setTimeout(() => searchInputRef.current?.focus(), 0)
              } else {
                setSearchQuery('')
              }
            }}
            title="Search"
          >
            <SearchIcon />
          </button>
          <button
            className="no-drag p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
            onClick={() => fetchSessions(true)}
            title="Refresh"
          >
            <RefreshIcon />
          </button>
        </div>
      </div>

      {/* Search bar */}
      {searchVisible && (
        <div className="px-3 py-2 border-b border-[var(--color-border)] flex flex-col gap-1.5 flex-shrink-0">
          <input
            ref={searchInputRef}
            type="text"
            className="no-drag w-full bg-white/[0.06] border border-[var(--color-border)] rounded px-2 py-1 text-[11px] font-mono text-[var(--color-text-secondary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-white/20"
            placeholder="Search chats..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
          />
          {searchQuery.trim() && (
            <button
              className="no-drag flex items-center justify-between w-full px-2 py-1 rounded text-[11px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors"
              onClick={handleAgenticSearch}
              title="Search agentically with your default agent (⌘↵)"
            >
              <span>Search Agentically</span>
              <span className="text-[10px] opacity-60">⌘↵</span>
            </button>
          )}
        </div>
      )}

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">Loading...</p>
          </div>
        ) : error ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-red-400 font-mono">Failed to load history</p>
          </div>
        ) : sessions.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No conversations yet
            </p>
          </div>
        ) : searchQuery.trim() && grouped.size === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No matching conversations
            </p>
          </div>
        ) : (
          <div className="py-1">
            {(() => {
              let flatIndex = 0
              return GROUP_ORDER.map((group) => {
                const items = grouped.get(group)
                if (!items || items.length === 0) return null

                return (
                  <div key={group} className="mb-1">
                    {/* Group header */}
                    <div className="px-3 py-1.5 border-b border-white/[0.04]">
                      <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] font-mono">
                        {group}
                      </span>
                    </div>

                    {/* Session rows */}
                    {items.map((session) => {
                      const idx = flatIndex++
                      const isSelected = searchVisible && idx === selectedIndex
                      return (
                        <button
                          key={`${session.provider}-${session.sessionId}`}
                          ref={isSelected ? selectedRowRef : undefined}
                          className={`no-drag w-full flex items-center gap-2 px-3 h-8 transition-colors text-left group ${
                            isSelected
                              ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
                              : 'hover:bg-white/[0.04] active:bg-white/[0.06]'
                          }`}
                          onClick={() => handleSessionClick(session)}
                        >
                          <ProviderIcon provider={session.provider} />

                          <div className="flex-1 min-w-0 flex flex-col justify-center">
                            <span className="text-[11px] text-[var(--color-text-secondary)] font-mono truncate leading-tight">
                              {session.title}
                            </span>
                            {session.messageCount > 0 && (
                              <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight">
                                {session.messageCount} message{session.messageCount !== 1 ? 's' : ''}
                              </span>
                            )}
                          </div>

                          <span className="text-[10px] text-[var(--color-text-muted)] font-mono flex-shrink-0 tabular-nums">
                            {formatTime(session.timestamp)}
                          </span>
                        </button>
                      )
                    })}
                  </div>
                )
              })
            })()}
          </div>
        )}
      </div>
    </div>
  )
}

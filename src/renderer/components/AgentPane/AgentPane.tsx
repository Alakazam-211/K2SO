import { useState, useEffect, useCallback, useRef, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AlacrittyTerminalView } from '@/components/Terminal/AlacrittyTerminalView'
import { RESUMABLE_CLI_TOOLS } from '@shared/constants'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

// ── Types ───────────────────────────────────────────────────────────────

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
  bodyPreview: string
}

interface AgentProfile {
  name: string
  role: string
  podLeader: boolean
  agentType: string
  raw: string
}

interface AgentPaneProps {
  agentName: string
  projectPath: string
  onClose?: () => void
}

// ── Helpers ─────────────────────────────────────────────────────────────

const priorityBadge = (p: string) => {
  const colors: Record<string, string> = {
    critical: 'bg-red-500/15 text-red-400',
    high: 'bg-orange-500/15 text-orange-400',
    normal: 'bg-white/5 text-[var(--color-text-muted)]',
    low: 'bg-white/5 text-[var(--color-text-muted)] opacity-60',
  }
  return colors[p] || colors.normal
}

// ── Kanban Card ─────────────────────────────────────────────────────────

function KanbanCard({ item, onClick }: { item: WorkItem; onClick: () => void }): React.JSX.Element {
  return (
    <div
      onClick={onClick}
      className="px-3 py-2.5 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)]/30 cursor-pointer transition-colors mb-2"
    >
      <div className="text-xs font-medium text-[var(--color-text-primary)] leading-snug">{item.title}</div>
      {item.bodyPreview && (
        <div className="text-[10px] text-[var(--color-text-muted)] leading-relaxed mt-1.5 line-clamp-2">{item.bodyPreview}</div>
      )}
      <div className="flex items-center gap-1.5 mt-2">
        <span className={`text-[9px] font-medium px-1.5 py-0.5 ${priorityBadge(item.priority)}`}>
          {item.priority}
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)]">{item.itemType}</span>
      </div>
      {item.assignedBy && item.assignedBy !== 'user' && item.assignedBy !== 'external' && item.assignedBy !== 'delegated' && (
        <div className="mt-2">
          <span className="text-[9px] font-medium px-1.5 py-0.5 bg-[var(--color-accent)]/10 text-[var(--color-accent)]">
            {item.assignedBy}
          </span>
        </div>
      )}
    </div>
  )
}

// ── Kanban Column ───────────────────────────────────────────────────────

function KanbanColumn({ title, items, color, agentDir, onOpenFile }: {
  title: string
  items: WorkItem[]
  color: string
  agentDir: string
  onOpenFile: (path: string) => void
}): React.JSX.Element {
  return (
    <div className="flex-1 min-w-0 flex flex-col">
      <div className="flex items-center gap-1.5 mb-2.5 px-1">
        <span className={`text-[10px] font-semibold uppercase tracking-wider ${color}`}>{title}</span>
        {items.length > 0 && (
          <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">
            {items.length}
          </span>
        )}
      </div>
      <div className="flex-1 overflow-y-auto px-0.5">
        {items.length === 0 ? (
          <div className="px-3 py-4 text-[11px] text-[var(--color-text-muted)] text-center border border-dashed border-[var(--color-border)]">
            None
          </div>
        ) : (
          items.map((item) => (
            <KanbanCard
              key={item.filename}
              item={item}
              onClick={() => onOpenFile(`${agentDir}/work/${item.folder}/${item.filename}`)}
            />
          ))
        )}
      </div>
    </div>
  )
}

// ── Agent Chat Terminal ─────────────────────────────────────────────────

function AgentChatTerminal({ agentName, agentDir, autoFocus }: { agentName: string; agentDir: string; autoFocus?: boolean }): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  // Stable terminal ID based on agent name — survives workspace stash/restore cycles
  // so the same PTY is reused and conversations aren't lost on workspace switch
  const terminalIdRef = useRef(`agent-chat-${agentName}`)
  const [resolvedArgs, setResolvedArgs] = useState<string[] | undefined>(undefined)
  const [ready, setReady] = useState(false)

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Detect previous session and build args
  useEffect(() => {
    let cancelled = false
    const resolve = async () => {
      const command = agentCommand?.command
      if (!command) {
        setReady(true)
        return
      }

      const baseArgs = [...(agentCommand?.args ?? [])]

      // Check for a previous chat session to resume
      const toolConfig = RESUMABLE_CLI_TOOLS[command]
      if (toolConfig) {
        try {
          const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
            provider: toolConfig.provider,
            projectPath: agentDir,
          })
          if (!cancelled && sessionId) {
            setResolvedArgs([...baseArgs, toolConfig.resumeFlag, sessionId])
            setReady(true)
            return
          }
        } catch { /* fall through to fresh session */ }
      }

      if (!cancelled) {
        setResolvedArgs(baseArgs)
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentCommand, agentDir])

  // NOTE: We intentionally do NOT kill the terminal on unmount.
  // When workspaces are stashed, React unmounts but PTYs stay alive in the
  // backend so conversations survive workspace switches. The terminal is
  // cleaned up by the backend when the app shuts down or via terminal_kill_all.

  // Auto-focus the terminal container when the chat tab becomes active
  useEffect(() => {
    if (autoFocus && ready) {
      requestAnimationFrame(() => {
        const el = containerRef.current?.querySelector('[tabindex]') as HTMLElement | null
        el?.focus()
      })
    }
  }, [autoFocus, ready])

  if (!agentCommand || !ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        {!agentCommand ? 'No AI agent configured. Set a default in Settings.' : 'Loading session...'}
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full">
      <AlacrittyTerminalView
        terminalId={terminalIdRef.current}
        cwd={agentDir}
        command={agentCommand.command}
        args={resolvedArgs}
      />
    </div>
  )
}

// Remember which tab each agent was on so navigating back doesn't reset
const lastActiveTab = new Map<string, 'chat' | 'profile' | 'claude-md' | 'work'>()

// ── Main Component ──────────────────────────────────────────────────────

export function AgentPane({ agentName, projectPath }: AgentPaneProps): React.JSX.Element {
  const isWorkspaceBoard = agentName === '__workspace__'

  const [profile, setProfile] = useState<AgentProfile | null>(null)
  const [claudeMd, setClaudeMd] = useState<string>('')
  const [workItems, setWorkItems] = useState<WorkItem[]>([])
  const [wsInboxItems, setWsInboxItems] = useState<WorkItem[]>([])
  const [allAgentWork, setAllAgentWork] = useState<WorkItem[]>([])
  const [viewMode, setViewMode] = useState<'preview' | 'edit'>('preview')
  const [activeSection, setActiveSection] = useState<'chat' | 'profile' | 'claude-md' | 'work'>(
    lastActiveTab.get(agentName) ?? 'work'
  )
  const [showPersonaEditor, setShowPersonaEditor] = useState(false)
  // Track whether the chat terminal has been mounted (lazy — only on first visit)
  const [chatMounted, setChatMounted] = useState(lastActiveTab.get(agentName) === 'chat')

  const agentDir = `${projectPath}/.k2so/agents/${agentName}`

  const fetchProfile = useCallback(async () => {
    if (isWorkspaceBoard) return
    try {
      const result = await invoke<string | { content: string }>('k2so_agents_get_profile', { projectPath, agentName })
      const raw = typeof result === 'string' ? result : (result.content || '')
      const fmMatch = raw.match(/^---\n([\s\S]*?)\n---/)
      let name = agentName, role = '', podLeader = false, agentType = 'pod-member'
      if (fmMatch) {
        const fm = fmMatch[1]
        name = fm.match(/^name:\s*(.+)$/m)?.[1]?.trim() || name
        role = fm.match(/^role:\s*(.+)$/m)?.[1]?.trim() || ''
        podLeader = fm.match(/^pod_leader:\s*(.+)$/m)?.[1]?.trim() === 'true'
        agentType = fm.match(/^type:\s*(.+)$/m)?.[1]?.trim() || 'pod-member'
      }
      setProfile({ name, role, podLeader, agentType, raw })
    } catch { setProfile(null) }
  }, [projectPath, agentName, isWorkspaceBoard])

  const fetchClaudeMd = useCallback(async () => {
    if (isWorkspaceBoard) return
    try {
      setClaudeMd(await invoke<string>('k2so_agents_generate_claude_md', { projectPath, agentName }))
    } catch { setClaudeMd('') }
  }, [projectPath, agentName, isWorkspaceBoard])

  const fetchWork = useCallback(async () => {
    if (isWorkspaceBoard) {
      try {
        const wsItems = await invoke<WorkItem[]>('k2so_agents_workspace_inbox_list', { projectPath })
        setWsInboxItems(wsItems)
      } catch { setWsInboxItems([]) }
      try {
        const agents = await invoke<{ name: string }[]>('k2so_agents_list', { projectPath })
        const all: WorkItem[] = []
        for (const agent of agents) {
          try {
            const items = await invoke<WorkItem[]>('k2so_agents_work_list', { projectPath, agentName: agent.name, folder: null })
            all.push(...items.map((i) => ({ ...i, assignedBy: agent.name })))
          } catch { /* skip */ }
        }
        setAllAgentWork(all)
      } catch { setAllAgentWork([]) }
    } else {
      try {
        setWorkItems(await invoke<WorkItem[]>('k2so_agents_work_list', { projectPath, agentName, folder: null }))
      } catch { setWorkItems([]) }
    }
  }, [projectPath, agentName, isWorkspaceBoard])

  useEffect(() => {
    fetchProfile(); fetchClaudeMd(); fetchWork()
    const interval = setInterval(fetchWork, 10_000)
    return () => clearInterval(interval)
  }, [fetchProfile, fetchClaudeMd, fetchWork])

  const openFile = (filePath: string) => useTabsStore.getState().openFileAsTab(filePath)

  // For single agent view
  const inbox = workItems.filter((w) => w.folder === 'inbox')
  const active = workItems.filter((w) => w.folder === 'active')
  const done = workItems.filter((w) => w.folder === 'done')

  // For workspace board view
  const wsUnassigned = wsInboxItems
  const wsInProgress = allAgentWork.filter((w) => w.folder === 'inbox' || w.folder === 'active')
  const wsReview = allAgentWork.filter((w) => w.folder === 'done')

  // Show the AIFileEditor persona editor when "Configure with AI" is clicked
  if (showPersonaEditor && !isWorkspaceBoard) {
    return (
      <div className="h-full">
        <AgentPersonaEditor
          agentName={agentName}
          projectPath={projectPath}
          onClose={() => {
            setShowPersonaEditor(false)
            fetchProfile()
          }}
        />
      </div>
    )
  }

  // Determine which tabs to show
  const showWork = profile ? (profile.agentType !== 'k2so' && profile.agentType !== 'custom') : false
  const showChat = !isWorkspaceBoard

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      {/* Header — tabs on left, agent name after */}
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        {/* Pill tabs on the left */}
        {!isWorkspaceBoard && (
          <div className="flex gap-0.5 flex-shrink-0">
            {(() => {
              const sections: Array<'chat' | 'work' | 'claude-md' | 'profile'> = showWork
                ? ['work', 'chat', 'claude-md', 'profile']
                : ['claude-md', 'chat', 'profile']
              return sections.map((section) => {
              const labels = { chat: 'Chat', work: 'Work', profile: 'Profile', 'claude-md': 'CLAUDE.md' }
              const isActive = activeSection === section
              return (
                <button
                  key={section}
                  onClick={() => {
                    setActiveSection(section)
                    lastActiveTab.set(agentName, section)
                    if (section === 'chat') setChatMounted(true)
                  }}
                  className={`px-3 py-1.5 text-[11px] font-medium transition-colors no-drag cursor-pointer ${
                    isActive
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                  }`}
                >
                  {labels[section]}
                </button>
              )
            })
            })()}
          </div>
        )}
        {/* Agent name + badge */}
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
            {isWorkspaceBoard ? 'Work Board' : agentName}
          </span>
          {profile?.podLeader && (
            <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 flex-shrink-0">
              LEADER
            </span>
          )}
        </div>
        {/* Preview/Edit toggle for profile/claude-md tabs */}
        {!isWorkspaceBoard && (activeSection === 'profile' || activeSection === 'claude-md') && (
          <div className="ml-auto flex gap-1.5 items-center flex-shrink-0">
            <div className="flex gap-0.5">
              {(['preview', 'edit'] as const).map((mode) => (
                <button
                  key={mode}
                  onClick={() => {
                    if (mode === 'edit') {
                      openFile(`${agentDir}/${activeSection === 'profile' ? 'agent.md' : 'CLAUDE.md'}`)
                    }
                    setViewMode(mode)
                  }}
                  className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                    viewMode === mode
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                  }`}
                >
                  {mode === 'preview' ? 'Preview' : 'Edit'}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Content — relative container so chat terminal can stay absolutely positioned
           and maintain its dimensions even when another tab is in front of it. */}
      <div className="flex-1 overflow-hidden min-h-0 relative">
        {/* ── Chat Terminal — always mounted at full size, layered behind when inactive ── */}
        {showChat && chatMounted && (
          <div className={`absolute inset-0 ${activeSection === 'chat' ? 'z-10' : 'z-0 pointer-events-none'}`}>
            <AgentChatTerminal agentName={agentName} agentDir={agentDir} autoFocus={activeSection === 'chat'} />
          </div>
        )}

        {/* ── Workspace Board (Kanban) ── */}
        {isWorkspaceBoard && (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto">
            <KanbanColumn title="Unassigned" items={wsUnassigned} color="text-[var(--color-accent)]" agentDir={`${projectPath}/.k2so/work`} onOpenFile={openFile} />
            <KanbanColumn title="In Progress" items={wsInProgress} color="text-yellow-400" agentDir={`${projectPath}/.k2so/agents`} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={wsReview} color="text-green-400" agentDir={`${projectPath}/.k2so/agents`} onOpenFile={openFile} />
          </div>
        )}

        {/* ── Agent Work Queue (Kanban) ── */}
        {!isWorkspaceBoard && activeSection === 'work' && (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
            <KanbanColumn title="Inbox" items={inbox} color="text-[var(--color-accent)]" agentDir={agentDir} onOpenFile={openFile} />
            <KanbanColumn title="Active" items={active} color="text-yellow-400" agentDir={agentDir} onOpenFile={openFile} />
            <KanbanColumn title="Done" items={done} color="text-green-400" agentDir={agentDir} onOpenFile={openFile} />
          </div>
        )}

        {/* ── Profile ── */}
        {!isWorkspaceBoard && activeSection === 'profile' && (
          <div className="absolute inset-0 z-10 overflow-y-auto overflow-x-hidden bg-[var(--color-bg)]">
            {viewMode === 'preview' ? (
              <div className="markdown-content p-4">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {profile?.raw || '*No agent.md found*'}
                </ReactMarkdown>
              </div>
            ) : (
              <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono p-4 leading-relaxed">
                {profile?.raw || 'No agent.md found'}
              </pre>
            )}
          </div>
        )}

        {/* ── CLAUDE.md ── */}
        {!isWorkspaceBoard && activeSection === 'claude-md' && (
          <div className="absolute inset-0 z-10 overflow-y-auto overflow-x-hidden bg-[var(--color-bg)]">
            {claudeMd ? (
              viewMode === 'preview' ? (
                <div className="markdown-content p-4">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>
                    {claudeMd}
                  </ReactMarkdown>
                </div>
              ) : (
                <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono p-4 leading-relaxed">
                  {claudeMd}
                </pre>
              )
            ) : (
              <div className="p-4">
                <p className="text-xs text-[var(--color-text-muted)]">No CLAUDE.md generated yet. Launch the agent to generate one.</p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

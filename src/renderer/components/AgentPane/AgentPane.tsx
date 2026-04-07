import { useState, useEffect, useCallback, useRef, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { addNavWorktree } from '@/components/Sidebar/Sidebar'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AlacrittyTerminalView } from '@/components/Terminal/AlacrittyTerminalView'
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
  isCoordinator: boolean // legacy field name from backend; true = manager agent
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

function KanbanColumn({ title, items, color, projectPath, agentDir, onOpenFile }: {
  title: string
  items: WorkItem[]
  color: string
  projectPath: string
  agentDir?: string  // for regular agent views
  onOpenFile: (path: string) => void
}): React.JSX.Element {
  const resolvePath = (item: WorkItem) => {
    // If item has assignedBy (from aggregated agent work), path is under that agent's dir
    if (item.assignedBy && item.assignedBy !== 'user' && item.assignedBy !== 'external' && item.assignedBy !== 'delegated') {
      return `${projectPath}/.k2so/agents/${item.assignedBy}/work/${item.folder}/${item.filename}`
    }
    // If agentDir is set (regular agent view), use it
    if (agentDir) {
      return `${agentDir}/work/${item.folder}/${item.filename}`
    }
    // Workspace-level items
    return `${projectPath}/.k2so/work/${item.folder}/${item.filename}`
  }

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
              onClick={() => onOpenFile(resolvePath(item))}
            />
          ))
        )}
      </div>
    </div>
  )
}

// ── Agent Chat Terminal ─────────────────────────────────────────────────

function AgentChatTerminal({ agentName, agentDir, projectPath, autoFocus }: { agentName: string; agentDir: string; projectPath: string; autoFocus?: boolean }): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  // Stable terminal ID based on agent name — survives workspace stash/restore cycles
  const terminalIdRef = useRef(`agent-chat-${agentName}`)
  const [launchConfig, setLaunchConfig] = useState<{
    command: string
    args: string[]
    cwd: string
  } | null>(null)
  const [ready, setReady] = useState(false)
  const [terminalKey, setTerminalKey] = useState(0)

  // Resolve terminal: check live PTY first, then use backend build_launch for resume/fresh
  useEffect(() => {
    let cancelled = false
    const resolve = async () => {
      const myTerminalId = terminalIdRef.current

      // Step 1: Check if our terminal already exists and is LIVE
      try {
        const exists = await invoke<boolean>('terminal_exists', { id: myTerminalId })
        if (!cancelled && exists) {
          // Terminal exists — reattach, don't create a new one
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

      // Step 2: Ask the backend for full launch config
      // k2so_agents_build_launch handles: DB session resume → .last_session → history.jsonl → fresh
      // Always uses --dangerously-skip-permissions and includes CLAUDE.md via --append-system-prompt
      try {
        const result = await invoke<{
          command: string
          args: string[]
          cwd: string
          resumeSession?: string
        }>('k2so_agents_build_launch', {
          projectPath: projectPath,
          agentName: agentName,
        })
        if (!cancelled && result) {
          setLaunchConfig({
            command: result.command,
            args: result.args,
            cwd: result.cwd,
          })
          setReady(true)
          return
        }
      } catch (err) {
        console.warn('[AgentChatTerminal] build_launch failed, falling back:', err)
      }

      // Step 3: Fallback — fresh session with just --dangerously-skip-permissions
      if (!cancelled) {
        setLaunchConfig({
          command: 'claude',
          args: ['--dangerously-skip-permissions'],
          cwd: agentDir,
        })
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, agentDir])

  // Auto-focus the terminal container when the chat tab becomes active
  useEffect(() => {
    if (autoFocus && ready) {
      requestAnimationFrame(() => {
        const el = containerRef.current?.querySelector('[tabindex]') as HTMLElement | null
        el?.focus()
      })
    }
  }, [autoFocus, ready])

  if (!ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session...
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full">
      <AlacrittyTerminalView
        key={terminalKey}
        terminalId={terminalIdRef.current}
        cwd={launchConfig?.cwd ?? agentDir}
        command={launchConfig?.command}
        args={launchConfig?.args}
      />
    </div>
  )
}

// Remember which tab each agent was on so navigating back doesn't reset
const lastActiveTab = new Map<string, 'chat' | 'profile' | 'claude-md' | 'work'>()

// ── Main Component ──────────────────────────────────────────────────────

export function AgentPane({ agentName, projectPath }: AgentPaneProps): React.JSX.Element {
  // Worktree views are a completely separate component — no shared hooks
  if (agentName.startsWith('__wt:')) {
    return <WorktreeDetailPane worktreeId={agentName.slice(5)} projectPath={projectPath} />
  }
  return <AgentPaneInner agentName={agentName} projectPath={projectPath} />
}

function AgentPaneInner({ agentName, projectPath }: AgentPaneProps): React.JSX.Element {
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
      let name = agentName, role = '', isCoordinator = false, agentType = 'agent-template'
      if (fmMatch) {
        const fm = fmMatch[1]
        name = fm.match(/^name:\s*(.+)$/m)?.[1]?.trim() || name
        role = fm.match(/^role:\s*(.+)$/m)?.[1]?.trim() || ''
        isCoordinator = fm.match(/^pod_leader:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^coordinator:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^manager:\s*(.+)$/m)?.[1]?.trim() === 'true'
        const rawType = fm.match(/^type:\s*(.+)$/m)?.[1]?.trim() || 'agent-template'
        agentType = rawType === 'pod-leader' || rawType === 'manager' ? 'coordinator' : rawType === 'pod-member' ? 'agent-template' : rawType
      }
      setProfile({ name, role, isCoordinator, agentType, raw })
    } catch { setProfile(null) }
  }, [projectPath, agentName, isWorkspaceBoard])

  const fetchClaudeMd = useCallback(async () => {
    if (isWorkspaceBoard) return
    try {
      setClaudeMd(await invoke<string>('k2so_agents_generate_claude_md', { projectPath, agentName }))
    } catch { setClaudeMd('') }
  }, [projectPath, agentName, isWorkspaceBoard])

  const isManager = profile?.isCoordinator || profile?.agentType === 'coordinator' || profile?.agentType === 'manager'

  const fetchWork = useCallback(async () => {
    if (isWorkspaceBoard || isManager) {
      // Manager and workspace board both see the full picture:
      // workspace inbox (unassigned) + all agents' work (delegated + review)
      try {
        const wsItems = await invoke<WorkItem[]>('k2so_agents_workspace_inbox_list', { projectPath })
        setWsInboxItems(wsItems)
      } catch { setWsInboxItems([]) }
      try {
        const agents = await invoke<{ name: string }[]>('k2so_agents_list', { projectPath })
        const all: WorkItem[] = []
        for (const agent of agents) {
          if (agent.name === agentName) continue // skip manager's own empty queue
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
  }, [projectPath, agentName, isWorkspaceBoard, isManager])

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
          {profile?.isCoordinator && (
            <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 flex-shrink-0">
              MANAGER
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
            <AgentChatTerminal agentName={agentName} agentDir={agentDir} projectPath={projectPath} autoFocus={activeSection === 'chat'} />
          </div>
        )}

        {/* ── Workspace Board (Kanban) ── */}
        {isWorkspaceBoard && (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto">
            <KanbanColumn title="Unassigned" items={wsUnassigned} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="In Progress" items={wsInProgress} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={wsReview} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        )}

        {/* ── Agent Work Queue (Kanban) ── */}
        {!isWorkspaceBoard && activeSection === 'work' && (
          isManager ? (
            // Manager sees: Inbox (workspace unassigned), Delegated (agents' inbox+active), Review (agents' done)
            <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
              <KanbanColumn title="Inbox" items={wsUnassigned} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
              <KanbanColumn title="Delegated" items={wsInProgress} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
              <KanbanColumn title="Review" items={wsReview} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
            </div>
          ) : (
            // Agent templates see their own work queue
            <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
              <KanbanColumn title="Inbox" items={inbox} color="text-[var(--color-accent)]" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
              <KanbanColumn title="Active" items={active} color="text-yellow-400" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
              <KanbanColumn title="Done" items={done} color="text-green-400" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
            </div>
          )
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

// ── Worktree Detail Pane ────────────────────────────────────────────────

const worktreeLastTab = new Map<string, 'task' | 'chat' | 'review'>()

function WorktreeDetailPane({ worktreeId, projectPath }: { worktreeId: string; projectPath: string }): React.JSX.Element {
  const [activeTab, setActiveTab] = useState<'task' | 'chat' | 'review'>(
    worktreeLastTab.get(worktreeId) ?? 'chat'
  )
  const [taskContent, setTaskContent] = useState<string>('')
  const [reviewContent, setReviewContent] = useState<string>('')
  const [chatMounted, setChatMounted] = useState(activeTab === 'chat')
  const [reviewAvailable, setReviewAvailable] = useState(false)

  // Look up workspace info from projects store (separate selectors to avoid new-object re-renders)
  const workspace = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      const ws = p.workspaces.find((w) => w.id === worktreeId)
      if (ws) return ws
    }
    return null
  }, [worktreeId]))
  const projectId = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      if (p.workspaces.some((w) => w.id === worktreeId)) return p.id
    }
    return null
  }, [worktreeId]))
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const displayName = workspace?.name?.replace(/^agent\/[^/]+\//, '') || workspace?.branch || 'Worktree'
  const agentMatch = workspace?.name?.match(/^agent\/([^/]+)\//)
  const agentTemplate = agentMatch?.[1]
  const worktreePath = workspace?.worktreePath || projectPath

  // Fetch task content (the delegated work item)
  useEffect(() => {
    if (!agentTemplate) {
      setTaskContent('')
      return
    }
    const loadTask = async () => {
      try {
        // Look for active work items for this agent template
        const items = await invoke<WorkItem[]>('k2so_agents_work_list', {
          projectPath,
          agentName: agentTemplate,
          folder: 'active',
        })
        if (items.length > 0) {
          // Read the first active task's full content
          const taskPath = `${projectPath}/.k2so/agents/${agentTemplate}/work/active/${items[0].filename}`
          const result = await invoke<{ content: string }>('fs_read_file', { path: taskPath })
          setTaskContent(result.content)
        } else {
          setTaskContent('')
        }
      } catch { setTaskContent('') }
    }
    loadTask()
  }, [projectPath, agentTemplate])

  // Fetch review content (done work items)
  useEffect(() => {
    if (!agentTemplate) {
      setReviewAvailable(false)
      return
    }
    const loadReview = async () => {
      try {
        const items = await invoke<WorkItem[]>('k2so_agents_work_list', {
          projectPath,
          agentName: agentTemplate,
          folder: 'done',
        })
        if (items.length > 0) {
          setReviewAvailable(true)
          const reviewPath = `${projectPath}/.k2so/agents/${agentTemplate}/work/done/${items[0].filename}`
          const result = await invoke<{ content: string }>('fs_read_file', { path: reviewPath })
          setReviewContent(result.content)
        } else {
          setReviewAvailable(false)
          setReviewContent('')
        }
      } catch {
        setReviewAvailable(false)
        setReviewContent('')
      }
    }
    loadReview()
  }, [projectPath, agentTemplate])

  // Mark chat as mounted when first visited
  useEffect(() => {
    if (activeTab === 'chat') setChatMounted(true)
  }, [activeTab])

  const tabs: Array<{ key: 'task' | 'chat' | 'review'; label: string; disabled: boolean }> = [
    { key: 'task', label: 'Task', disabled: !agentTemplate || !taskContent },
    { key: 'chat', label: 'Chat', disabled: false },
    { key: 'review', label: 'Review', disabled: !reviewAvailable },
  ]

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      {/* Header */}
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <div className="flex gap-0.5 flex-shrink-0">
          {tabs.map(({ key, label, disabled }) => (
            <button
              key={key}
              onClick={() => {
                if (!disabled) {
                  setActiveTab(key)
                  worktreeLastTab.set(worktreeId, key)
                }
              }}
              disabled={disabled}
              className={`px-3 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                activeTab === key
                  ? 'bg-[var(--color-accent)] text-white'
                  : disabled
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)]/40 cursor-not-allowed'
                    : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {displayName}
        </span>
        {agentTemplate && (
          <span className="text-[9px] font-medium text-[var(--color-text-muted)] bg-white/5 px-1.5 py-0.5 flex-shrink-0">
            {agentTemplate}
          </span>
        )}

        <div className="flex-1" />

        {projectId && (
          <button
            onClick={() => {
              // Close this worktree tab first, then switch to the full workspace
              const currentTabId = useTabsStore.getState().activeTabId
              if (currentTabId) {
                useTabsStore.getState().removeTab(currentTabId)
              }
              // Add to nav permanently, then switch to the worktree workspace
              addNavWorktree(worktreeId)
              setTimeout(() => setActiveWorkspace(projectId, worktreeId), 50)
            }}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0 flex items-center gap-1"
            title="Open this worktree as a full workspace with file tree and changes"
          >
            <svg className="w-3 h-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              <path d="M7 2H3a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V9" />
              <path d="M14 2l-7 7" />
              <path d="M10 2h4v4" />
            </svg>
            Open Full Workspace
          </button>
        )}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden relative">
        {/* Task tab */}
        {activeTab === 'task' && (
          <div className="h-full overflow-y-auto p-4">
            {taskContent ? (
              <div className="prose prose-sm prose-invert max-w-none">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{taskContent}</ReactMarkdown>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  {agentTemplate ? 'No active task assigned.' : 'This worktree was not created by the agent system.'}
                </p>
              </div>
            )}
          </div>
        )}

        {/* Chat tab — lazy mount, absolute positioning to preserve terminal state */}
        {chatMounted && (
          <div
            className="absolute inset-0"
            style={{ zIndex: activeTab === 'chat' ? 1 : 0, visibility: activeTab === 'chat' ? 'visible' : 'hidden' }}
          >
            <AgentChatTerminal
              agentName={`wt-${worktreeId}`}
              agentDir={worktreePath}
              projectPath={projectPath}
              autoFocus={activeTab === 'chat'}
            />
          </div>
        )}

        {/* Review tab */}
        {activeTab === 'review' && (
          <div className="h-full overflow-y-auto p-4">
            {reviewAvailable ? (
              <div className="space-y-4">
                <div className="prose prose-sm prose-invert max-w-none">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{reviewContent}</ReactMarkdown>
                </div>

                <div className="border-t border-[var(--color-border)] pt-4 space-y-3">
                  <button
                    className="w-full px-4 py-2 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
                    onClick={async () => {
                      try {
                        await invoke('git_merge_worktree', { projectPath, workspaceId: worktreeId })
                      } catch (e) {
                        console.error('[worktree] Merge failed:', e)
                      }
                    }}
                  >
                    AI Merge Worktree/Branch
                  </button>
                  <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                    If the work is not right, go to the Chat tab to address the issue with the agent before merging.
                  </p>
                </div>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  No completed work to review yet.
                </p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}


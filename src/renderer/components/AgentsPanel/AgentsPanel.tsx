import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

interface K2soAgentInfo {
  name: string
  role: string
  inboxCount: number
  activeCount: number
  doneCount: number
  podLeader: boolean
}

export default function AgentsPanel(): React.JSX.Element {
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [wsInboxCount, setWsInboxCount] = useState(0)
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [newName, setNewName] = useState('')
  const [newRole, setNewRole] = useState('')
  const [creating, setCreating] = useState(false)
  const nameInputRef = useRef<HTMLInputElement>(null)

  const activeProject = useProjectsStore((s) => {
    const id = s.activeProjectId
    return id ? s.projects.find((p) => p.id === id) : null
  })

  const fetchAgents = useCallback(async () => {
    if (!activeProject) {
      setAgents([])
      setLoading(false)
      return
    }
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', {
        projectPath: activeProject.path,
      })
      setAgents(result)
    } catch {
      setAgents([])
    } finally {
      setLoading(false)
    }
  }, [activeProject])

  const fetchWsInbox = useCallback(async () => {
    if (!activeProject) return
    try {
      const items = await invoke<unknown[]>('k2so_agents_workspace_inbox_list', { projectPath: activeProject.path })
      setWsInboxCount(items.length)
    } catch {
      setWsInboxCount(0)
    }
  }, [activeProject])

  useEffect(() => {
    fetchAgents()
    fetchWsInbox()
    const interval = setInterval(() => { fetchAgents(); fetchWsInbox() }, 10_000)
    return () => clearInterval(interval)
  }, [fetchAgents, fetchWsInbox])

  useEffect(() => {
    if (showCreate) {
      requestAnimationFrame(() => nameInputRef.current?.focus())
    }
  }, [showCreate])

  const handleCreate = useCallback(async () => {
    if (!activeProject || !newName.trim() || !newRole.trim()) return
    setCreating(true)
    try {
      await invoke('k2so_agents_create', {
        projectPath: activeProject.path,
        name: newName.trim().toLowerCase().replace(/\s+/g, '-'),
        role: newRole.trim(),
      })
      setNewName('')
      setNewRole('')
      setShowCreate(false)
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Create failed:', e)
    } finally {
      setCreating(false)
    }
  }, [activeProject, newName, newRole, fetchAgents])

  const handleLaunch = useCallback(async (agentName: string) => {
    if (!activeProject) return
    try {
      const launchInfo = await invoke<{
        command: string
        args: string[]
        cwd: string
        agentName: string
      }>('k2so_agents_build_launch', {
        projectPath: activeProject.path,
        agentName,
      })
      useTabsStore.getState().addTab(launchInfo.cwd, {
        title: `Agent: ${launchInfo.agentName}`,
        command: launchInfo.command,
        args: launchInfo.args,
      })
    } catch (e) {
      console.error('[agents] Launch failed:', e)
    }
  }, [activeProject])

  const handleDelete = useCallback(async (name: string) => {
    if (!activeProject) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Delete Agent "${name}"?`,
      message: 'This will delete the agent and all its work items.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await invoke('k2so_agents_delete', { projectPath: activeProject.path, name })
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Delete failed:', e)
    }
  }, [activeProject, fetchAgents])

  // Status dot color based on work state
  const statusColor = (agent: K2soAgentInfo): string => {
    if (agent.activeCount > 0) return '#eab308' // yellow — working
    if (agent.doneCount > 0) return '#22c55e'   // green — has completed work
    if (agent.inboxCount > 0) return '#3b82f6'  // blue — has pending work
    return '#6b7280'                              // gray — idle
  }

  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  if (!agenticEnabled) {
    return (
      <div className="h-full flex flex-col items-center justify-center p-4 gap-2">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          Agentic Systems is off
        </p>
        <p className="text-[9px] text-[var(--color-text-muted)] text-center">
          Enable it in Settings &gt; General
        </p>
      </div>
    )
  }

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  const agentMode = activeProject.agentMode || 'off'

  if (agentMode === 'off') {
    return (
      <div className="h-full flex flex-col items-center justify-center p-4 gap-2">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          No agent mode enabled for this workspace
        </p>
        <p className="text-[9px] text-[var(--color-text-muted)] text-center">
          Enable Agent or Pod mode in workspace settings
        </p>
      </div>
    )
  }

  if (agentMode === 'agent') {
    return (
      <div className="h-full flex flex-col p-4 gap-3">
        <div>
          <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">Agent Mode</span>
          <p className="text-[10px] text-[var(--color-text-secondary)] mt-1">
            This workspace operates as a single AI agent. Open a Claude terminal to start working.
          </p>
        </div>
        <button
          onClick={async () => {
            try {
              const launchInfo = await invoke<{
                command: string; args: string[]; cwd: string; agentName: string
              }>('k2so_agents_build_launch', {
                projectPath: activeProject.path,
                agentName: activeProject.name.toLowerCase().replace(/\s+/g, '-'),
              })
              useTabsStore.getState().addTab(launchInfo.cwd, {
                title: `Agent: ${activeProject.name}`,
                command: launchInfo.command,
                args: launchInfo.args,
              })
            } catch (e) {
              console.error('[agents] Launch failed:', e)
            }
          }}
          className="px-3 py-1.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
        >
          Launch Agent
        </button>
        <WorkspaceInboxSummary projectPath={activeProject.path} />
      </div>
    )
  }

  // Pod mode — show pod leader + pod leader + pod members

  const podLeader = agents.find((a) => a.podLeader)
  const podMembers = agents.filter((a) => !a.podLeader)
  const totalDelegated = podMembers.reduce((sum, a) => sum + a.inboxCount + a.activeCount, 0)
  const totalDone = podMembers.reduce((sum, a) => sum + a.doneCount, 0)

  const openAgentPane = (name: string) => {
    if (!activeProject) return
    useTabsStore.getState().openAgentPane(name, activeProject.path)
  }

  const AgentRow = ({ agent, showDelete = true }: { agent: K2soAgentInfo; showDelete?: boolean }) => (
    <div
      className="px-3 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] transition-colors group cursor-pointer"
      onClick={() => openAgentPane(agent.name)}
    >
      <div className="flex items-center gap-2">
        <span
          className="w-1.5 h-1.5 rounded-full flex-shrink-0"
          style={{ backgroundColor: statusColor(agent) }}
          title={
            agent.activeCount > 0 ? 'Working' :
            agent.doneCount > 0 ? 'Has completed work' :
            agent.inboxCount > 0 ? 'Has pending work' : 'Idle'
          }
        />
        <div className="flex-1 min-w-0">
          <div className="text-xs text-[var(--color-text-primary)] truncate">
            {agent.name}
            {agent.podLeader && (
              <span className="ml-1.5 text-[9px] text-[var(--color-accent)] font-medium">LEADER</span>
            )}
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] truncate">{agent.role}</div>
        </div>
        <div className="flex items-center gap-1 text-[9px] text-[var(--color-text-muted)] flex-shrink-0">
          {agent.inboxCount > 0 && <span title="Inbox">{agent.inboxCount}i</span>}
          {agent.activeCount > 0 && <span className="text-yellow-400" title="Active">{agent.activeCount}a</span>}
          {agent.doneCount > 0 && <span className="text-green-400" title="Done">{agent.doneCount}d</span>}
        </div>
        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0">
          <button
            onClick={(e) => { e.stopPropagation(); handleLaunch(agent.name) }}
            className="px-1.5 py-0.5 text-[9px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer"
            title="Launch agent session"
          >
            ▶
          </button>
          {showDelete && (
            <button
              onClick={(e) => { e.stopPropagation(); handleDelete(agent.name) }}
              className="px-1 py-0.5 text-[9px] text-red-400/50 hover:text-red-400 hover:bg-red-500/10 no-drag cursor-pointer"
              title="Delete agent"
            >
              ×
            </button>
          )}
        </div>
      </div>
    </div>
  )

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Pod Leader section */}
      {podLeader && (
        <div className="border-b border-[var(--color-border)]">
          <div className="px-3 py-1.5">
            <span className="text-[9px] font-medium text-[var(--color-accent)] uppercase tracking-wider">Pod Leader</span>
          </div>
          <div className="px-3 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] transition-colors group">
            <div className="flex items-center gap-2">
              <span
                className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                style={{ backgroundColor: statusColor(podLeader) }}
              />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{podLeader.name}</span>
                  <span className="text-[9px] font-medium text-[var(--color-accent)]">LEADER</span>
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] truncate">{podLeader.role}</div>
              </div>
              <div className="flex items-center gap-1 text-[9px] text-[var(--color-text-muted)] flex-shrink-0">
                {wsInboxCount > 0 && <span className="text-[var(--color-accent)]" title="Undelegated">{wsInboxCount}u</span>}
                {totalDelegated > 0 && <span className="text-yellow-400" title="Delegated">{totalDelegated}d</span>}
                {totalDone > 0 && <span className="text-green-400" title="Done">{totalDone}✓</span>}
              </div>
              <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0">
                <button
                  onClick={() => handleLaunch(podLeader.name)}
                  className="px-1.5 py-0.5 text-[9px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer"
                  title="Launch pod leader session"
                >
                  ▶
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Pod Members header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
          Agents
          {podMembers.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{podMembers.length}</span>
          )}
        </span>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          title="New Agent"
        >
          {showCreate ? '×' : '+'}
        </button>
      </div>

      {/* Create form */}
      {showCreate && (
        <div className="px-3 py-2 border-b border-[var(--color-border)] space-y-1.5">
          <input
            ref={nameInputRef}
            type="text"
            placeholder="Name (e.g. backend-eng)"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[10px] text-[var(--color-text-primary)] px-2 py-1 outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
          />
          <input
            type="text"
            placeholder="Role (e.g. Backend API development)"
            value={newRole}
            onChange={(e) => setNewRole(e.target.value)}
            className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[10px] text-[var(--color-text-primary)] px-2 py-1 outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
          />
          <button
            onClick={handleCreate}
            disabled={creating || !newName.trim() || !newRole.trim()}
            className="w-full px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer disabled:opacity-50"
          >
            {creating ? 'Creating...' : 'Create'}
          </button>
        </div>
      )}

      {/* Agent list */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">Loading...</p>
          </div>
        ) : podMembers.length === 0 ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">
              No agents yet. Click + to create one.
            </p>
          </div>
        ) : (
          podMembers.map((agent) => <AgentRow key={agent.name} agent={agent} />)
        )}
      </div>

      {/* Workspace inbox summary */}
      <WorkspaceInboxSummary projectPath={activeProject.path} />
    </div>
  )
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
}

function WorkspaceInboxSummary({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [items, setItems] = useState<WorkItem[]>([])
  const [expanded, setExpanded] = useState(true)

  useEffect(() => {
    const check = async () => {
      try {
        const result = await invoke<WorkItem[]>('k2so_agents_workspace_inbox_list', { projectPath })
        setItems(result)
      } catch {
        setItems([])
      }
    }
    check()
    const interval = setInterval(check, 15_000)
    return () => clearInterval(interval)
  }, [projectPath])

  if (items.length === 0) return null

  const openWorkItem = (item: WorkItem) => {
    const filePath = `${projectPath}/.k2so/work/inbox/${item.filename}`
    const tab = useTabsStore.getState().getActiveTab()
    if (tab) {
      useTabsStore.getState().openFileInPane(tab.id, filePath)
    }
  }

  const priorityColor = (p: string) => {
    if (p === 'critical') return 'text-red-400'
    if (p === 'high') return 'text-orange-400'
    return 'text-[var(--color-text-muted)]'
  }

  return (
    <div className="border-t border-[var(--color-border)] flex-shrink-0">
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full px-3 py-1.5 flex items-center justify-between bg-[var(--color-bg-elevated)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer"
      >
        <span className="text-[9px] font-medium text-[var(--color-accent)] uppercase tracking-wider">
          Workspace Inbox ({items.length})
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)]">{expanded ? '▾' : '▸'}</span>
      </button>
      {expanded && (
        <div className="max-h-[200px] overflow-y-auto">
          {items.map((item) => (
            <div
              key={item.filename}
              onClick={() => openWorkItem(item)}
              className="px-3 py-1.5 border-t border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] cursor-pointer transition-colors"
            >
              <div className="flex items-center justify-between">
                <span className="text-[10px] text-[var(--color-text-primary)] truncate flex-1 mr-2">{item.title}</span>
                <span className={`text-[9px] flex-shrink-0 ${priorityColor(item.priority)}`}>{item.priority}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

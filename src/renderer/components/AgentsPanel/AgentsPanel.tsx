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
}

export default function AgentsPanel(): React.JSX.Element {
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
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

  useEffect(() => {
    fetchAgents()
    const interval = setInterval(fetchAgents, 10_000)
    return () => clearInterval(interval)
  }, [fetchAgents])

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

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  if (!activeProject.agentEnabled) {
    return (
      <div className="h-full flex flex-col items-center justify-center p-4 gap-2">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          Agent mode is off for this workspace
        </p>
        <button
          onClick={async () => {
            await invoke('projects_update', { id: activeProject.id, agentEnabled: 1 })
            await invoke('k2so_agents_generate_workspace_claude_md', { projectPath: activeProject.path }).catch(console.error)
            useProjectsStore.getState().fetchProjects()
          }}
          className="px-3 py-1 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
        >
          Enable Agent Mode
        </button>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
          Agents {agents.length > 0 && `(${agents.length})`}
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
            className="w-full px-2 py-1 text-[10px] font-medium bg-purple-600 text-white hover:bg-purple-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
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
        ) : agents.length === 0 ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">
              No agents yet. Click + to create one.
            </p>
          </div>
        ) : (
          agents.map((agent) => (
            <div
              key={agent.name}
              className="px-3 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] transition-colors group"
            >
              {/* Agent row */}
              <div className="flex items-center gap-2">
                {/* Status dot */}
                <span
                  className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                  style={{ backgroundColor: statusColor(agent) }}
                  title={
                    agent.activeCount > 0 ? 'Working' :
                    agent.doneCount > 0 ? 'Has completed work' :
                    agent.inboxCount > 0 ? 'Has pending work' : 'Idle'
                  }
                />

                {/* Name + role */}
                <div className="flex-1 min-w-0">
                  <div className="text-xs text-[var(--color-text-primary)] truncate">
                    {agent.name}
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] truncate">
                    {agent.role}
                  </div>
                </div>

                {/* Work counts */}
                <div className="flex items-center gap-1 text-[9px] text-[var(--color-text-muted)] flex-shrink-0">
                  {agent.inboxCount > 0 && <span title="Inbox">{agent.inboxCount}i</span>}
                  {agent.activeCount > 0 && <span className="text-yellow-400" title="Active">{agent.activeCount}a</span>}
                  {agent.doneCount > 0 && <span className="text-green-400" title="Done">{agent.doneCount}d</span>}
                </div>

                {/* Actions (visible on hover) */}
                <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0">
                  <button
                    onClick={() => handleLaunch(agent.name)}
                    className="px-1.5 py-0.5 text-[9px] text-purple-400 hover:text-purple-300 hover:bg-purple-500/10 no-drag cursor-pointer"
                    title="Launch agent session"
                  >
                    ▶
                  </button>
                  <button
                    onClick={() => handleDelete(agent.name)}
                    className="px-1 py-0.5 text-[9px] text-red-400/50 hover:text-red-400 hover:bg-red-500/10 no-drag cursor-pointer"
                    title="Delete agent"
                  >
                    ×
                  </button>
                </div>
              </div>
            </div>
          ))
        )}
      </div>

      {/* Workspace inbox summary */}
      <WorkspaceInboxSummary projectPath={activeProject.path} />
    </div>
  )
}

function WorkspaceInboxSummary({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [count, setCount] = useState(0)

  useEffect(() => {
    const check = async () => {
      try {
        const items = await invoke<unknown[]>('k2so_agents_workspace_inbox_list', { projectPath })
        setCount(items.length)
      } catch {
        setCount(0)
      }
    }
    check()
    const interval = setInterval(check, 15_000)
    return () => clearInterval(interval)
  }, [projectPath])

  if (count === 0) return null

  return (
    <div className="px-3 py-1.5 border-t border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
      <span className="text-[10px] text-[var(--color-accent)]">
        {count} item{count !== 1 ? 's' : ''} in workspace inbox
      </span>
    </div>
  )
}

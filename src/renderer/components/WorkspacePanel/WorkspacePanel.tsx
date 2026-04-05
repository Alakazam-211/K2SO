import { useState, useEffect, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useActiveAgentsStore, type PaneStatus } from '@/stores/active-agents'
import { showContextMenu } from '@/lib/context-menu'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'

// ── Types ────────────────────────────────────────────────────────────────

interface StateData {
  id: string
  name: string
  description: string | null
  isBuiltIn: number
  capFeatures: string
  capIssues: string
  capCrashes: string
  capSecurity: string
  capAudits: string
  heartbeat: number
  sortOrder: number
}

interface K2soAgentInfo {
  name: string
  role: string
  inboxCount: number
  activeCount: number
  doneCount: number
  isCoordinator: boolean
  agentType: string
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
  bodyPreview: string
  source: string
}

// ── Helpers ──────────────────────────────────────────────────────────────

function statusColor(status: PaneStatus): string {
  switch (status) {
    case 'working': return '#3b82f6'
    case 'permission': return '#ef4444'
    case 'review': return '#22c55e'
    default: return '#6b7280'
  }
}

function statusLabel(status: PaneStatus): string {
  switch (status) {
    case 'working': return 'Working'
    case 'permission': return 'Needs Permission'
    case 'review': return 'Review Ready'
    default: return 'Idle'
  }
}

const modeLabels: Record<string, string> = {
  off: 'Off',
  custom: 'Custom Agent',
  agent: 'K2SO Agent',
  coordinator: 'Coordinator',
  pod: 'Coordinator', // legacy
}

// ── Component ────────────────────────────────────────────────────────────

export default function WorkspacePanel(): React.JSX.Element {
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [wsInboxCount, setWsInboxCount] = useState(0)
  const [showWorktreeDialog, setShowWorktreeDialog] = useState(false)
  const [states, setStates] = useState<StateData[]>([])

  // Fetch workspace states once
  useEffect(() => {
    invoke<StateData[]>('states_list').then(setStates).catch(() => {})
  }, [])

  // Use stable selectors — avoid creating new references on every store change
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeProject = useProjectsStore(useCallback((s) => {
    return s.activeProjectId ? s.projects.find((p) => p.id === s.activeProjectId) ?? null : null
  }, []))
  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  const openAgentPane = useTabsStore((s) => s.openAgentPane)
  const projectStatus = useActiveAgentsStore(useCallback((s) =>
    activeProjectId ? s.getProjectStatus(activeProjectId) : 'idle' as PaneStatus
  , [activeProjectId]))

  // Fetch agents — keyed on stable ID, not object reference
  const activeProjectPath = activeProject?.path
  useEffect(() => {
    if (!activeProjectId || !activeProjectPath) return
    let cancelled = false
    const load = async (): Promise<void> => {
      try {
        const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', { projectPath: activeProjectPath })
        if (!cancelled) setAgents(result)
      } catch { if (!cancelled) setAgents([]) }
      try {
        const items = await invoke<WorkItem[]>('k2so_agents_workspace_inbox_list', {
          projectPath: activeProjectPath,
        })
        if (!cancelled) setWsInboxCount(items.length)
      } catch { if (!cancelled) setWsInboxCount(0) }
    }
    load()
    const interval = setInterval(load, 30000) // 30s, not 15s — reduce IPC chatter
    return () => { cancelled = true; clearInterval(interval) }
  }, [activeProjectId, activeProjectPath])

  const agentMode = activeProject?.agentMode || 'off'
  const isCoordinatorMode = agentMode === 'coordinator' || agentMode === 'pod'
  // Primary agent for any mode: coordinator for coordinator mode, first agent for custom/k2so
  const primaryAgent = useMemo(() => {
    if (isCoordinatorMode) return agents.find((a) => a.isCoordinator) ?? null
    return agents.length > 0 ? agents[0] : null
  }, [agents, isCoordinatorMode])
  const workspaces = activeProject?.workspaces ?? []
  // Filter to only worktree workspaces (not the main workspace)
  const worktrees = useMemo(() =>
    workspaces.filter((ws) => ws.worktreePath && ws.type !== 'main'),
    [workspaces]
  )

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          No workspace selected
        </p>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* ── Status ── */}
      <div className="px-3 py-3 border-b border-[var(--color-border)]">
        {/* Mode + status */}
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span
              className="w-2.5 h-2.5 rounded-full flex-shrink-0"
              style={{ backgroundColor: statusColor(projectStatus) }}
            />
            <span className="text-xs font-medium text-[var(--color-text-primary)]">
              {modeLabels[agentMode] || agentMode}
            </span>
            {projectStatus !== 'idle' && (
              <span className="text-[11px] text-[var(--color-text-muted)]">
                — {statusLabel(projectStatus)}
              </span>
            )}
          </div>
          {wsInboxCount > 0 && (
            <span className="text-[11px] text-[var(--color-accent)]">
              {wsInboxCount} inbox
            </span>
          )}
        </div>

        {/* Agent row — clickable to open agent pane */}
        {agentMode !== 'off' && primaryAgent && (
          <div
            className="mt-2.5 px-2 py-1.5 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer -mx-1"
            onClick={() => openAgentPane(primaryAgent.name, activeProject.path)}
          >
            <div className="flex items-center gap-2">
              <span
                className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                style={{ backgroundColor: statusColor(projectStatus) }}
              />
              <div className="flex-1 min-w-0">
                <span className="text-[11px] text-[var(--color-text-primary)] truncate block">{primaryAgent.name}</span>
                <span className="text-[10px] text-[var(--color-text-muted)] truncate block">{primaryAgent.role}</span>
              </div>
            </div>
          </div>
        )}

        {/* Off mode inbox shortcut */}
        {agentMode === 'off' && wsInboxCount > 0 && (
          <div
            className="mt-2.5 px-2 py-1.5 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer -mx-1"
            onClick={() => openAgentPane('__workspace__', activeProject.path)}
          >
            <div className="flex items-center gap-2">
              <svg className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
                <rect x="3" y="3" width="18" height="18" rx="2" />
                <path d="M3 9h18" />
              </svg>
              <span className="text-[11px] text-[var(--color-text-primary)]">View Inbox</span>
            </div>
          </div>
        )}

        {/* Heartbeat & State — only for AI-assisted modes */}
        {agentMode !== 'off' && (
          <>
            <div className="flex items-center justify-between mt-3.5">
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-[var(--color-text-secondary)]">Heartbeat</span>
                {activeProject.heartbeatEnabled === 1 && (
                  <span className="text-[11px] text-red-400 animate-pulse">♥</span>
                )}
              </div>
              <button
                onClick={async () => {
                  const newVal = activeProject.heartbeatEnabled ? 0 : 1
                  const store = useProjectsStore.getState()
                  const updated = store.projects.map((p) =>
                    p.id === activeProject.id ? { ...p, heartbeatEnabled: newVal } : p
                  )
                  useProjectsStore.setState({ projects: updated })
                  await invoke('projects_update', { id: activeProject.id, heartbeatEnabled: newVal })
                  await invoke('k2so_agents_update_heartbeat_projects').catch(console.error)
                  if (newVal === 1) {
                    await invoke('k2so_agents_install_heartbeat').catch(console.error)
                  }
                }}
                className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer ${
                  activeProject.heartbeatEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-2.5 h-2.5 bg-white block transition-transform ${
                    activeProject.heartbeatEnabled ? 'translate-x-3.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>

            {states.length > 0 && (
              <div className="flex items-center justify-between mt-3.5">
                <span className="text-[11px] text-[var(--color-text-secondary)]">State</span>
                <button
                  onClick={async () => {
                    const menuItems = [
                      { id: '__none__', label: 'No state' },
                      { id: '__sep__', label: '', type: 'separator' as const },
                      ...states.map((s) => ({ id: s.id, label: s.name })),
                    ]
                    const clickedId = await showContextMenu(menuItems)
                    if (clickedId === null) return
                    const stateId = clickedId === '__none__' ? '' : clickedId
                    try {
                      await invoke('projects_update', { id: activeProject.id, stateId: stateId || '' })
                      const store = useProjectsStore.getState()
                      const updated = store.projects.map((p) =>
                        p.id === activeProject.id ? { ...p, stateId: stateId || null } : p
                      )
                      useProjectsStore.setState({ projects: updated })
                    } catch (err) {
                      console.error('[workspace-panel] State update failed:', err)
                    }
                  }}
                  className="text-[11px] text-[var(--color-text-primary)] hover:text-[var(--color-accent)] transition-colors cursor-pointer no-drag flex items-center gap-1 border border-[var(--color-border)] px-2 py-0.5"
                >
                  <span>{states.find((s) => s.id === activeProject.stateId)?.name || 'No state'}</span>
                  <svg className="w-2.5 h-2.5 text-[var(--color-text-muted)]" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <path d="M2.5 4L5 6.5L7.5 4" />
                  </svg>
                </button>
              </div>
            )}
          </>
        )}
      </div>

      {/* ── Worktrees ── */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
          Worktrees
          {worktrees.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-white/5 text-[var(--color-text-muted)]">
              {worktrees.length}
            </span>
          )}
        </span>
        <button
          onClick={() => setShowWorktreeDialog(true)}
          className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10 transition-colors no-drag cursor-pointer"
          title="New worktree"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="5" y1="1" x2="5" y2="9" />
            <line x1="1" y1="5" x2="9" y2="5" />
          </svg>
        </button>
      </div>

      {showWorktreeDialog && (
        <WorktreeDialog
          projectId={activeProject.id}
          projectPath={activeProject.path}
          open={true}
          onClose={() => setShowWorktreeDialog(false)}
        />
      )}

      <div className="flex-1 overflow-y-auto">
        {worktrees.length === 0 ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">
              No worktrees open. Click + to create one or let the coordinator delegate work.
            </p>
          </div>
        ) : (
          worktrees.map((ws) => {
            // Strip agent/<name>/ prefix from display name
            const displayName = ws.name?.replace(/^agent\/[^/]+\//, '') || ws.branch || 'worktree'
            // Check if this was created by our agent system
            const agentMatch = ws.name?.match(/^agent\/([^/]+)\//)
            const agentTemplate = agentMatch?.[1]

            return (
              <WorktreeRow
                key={ws.id}
                workspaceId={ws.id}
                projectId={activeProject.id}
                projectPath={activeProject.path}
                worktreePath={ws.worktreePath}
                displayName={displayName}
                branch={ws.branch}
                agentTemplate={agentTemplate}
              />
            )
          })
        )}
      </div>
    </div>
  )
}

// ── WorktreeRow ──────────────────────────────────────────────────────────

function WorktreeRow({
  workspaceId,
  projectId,
  projectPath,
  worktreePath,
  displayName,
  branch,
  agentTemplate,
}: {
  workspaceId: string
  projectId: string
  projectPath: string
  worktreePath: string | null
  displayName: string
  branch: string | null
  agentTemplate?: string
}): React.JSX.Element {
  const openAgentPane = useTabsStore((s) => s.openAgentPane)
  const tabTitle = displayName || branch || 'Worktree'

  const handleContextMenu = useCallback(async (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()

    const menuItems = [
      { id: 'open', label: 'Open' },
      { id: 'open-finder', label: 'Show in Finder' },
      { id: 'separator-1', label: '', type: 'separator' },
      { id: 'close', label: 'Close Worktree' },
      { id: 'recycle', label: 'Recycle Worktree' },
    ]

    const clickedId = await showContextMenu(menuItems)

    if (clickedId === 'open') {
      openAgentPane(`__wt:${workspaceId}`, projectPath, tabTitle)
    } else if (clickedId === 'open-finder' && worktreePath) {
      await invoke('projects_open_in_finder', { path: worktreePath })
    } else if (clickedId === 'close') {
      // Remove from DB, keep files on disk
      await invoke('workspaces_delete', { id: workspaceId })
      // Optimistic removal from store
      const state = useProjectsStore.getState()
      const updated = state.projects.map((p) => {
        if (p.id !== projectId) return p
        return { ...p, workspaces: p.workspaces.filter((ws) => ws.id !== workspaceId) }
      })
      useProjectsStore.setState({ projects: updated })
    } else if (clickedId === 'recycle') {
      // Remove git worktree from disk + remove from DB
      try {
        if (worktreePath) {
          await invoke('git_remove_worktree', {
            projectPath,
            worktreePath,
            workspaceId,
          })
        } else {
          await invoke('workspaces_delete', { id: workspaceId })
        }
      } catch {
        // If git remove fails, just delete the record
        await invoke('workspaces_delete', { id: workspaceId })
      }
      // Optimistic removal from store
      const state = useProjectsStore.getState()
      const updated = state.projects.map((p) => {
        if (p.id !== projectId) return p
        return { ...p, workspaces: p.workspaces.filter((ws) => ws.id !== workspaceId) }
      })
      useProjectsStore.setState({ projects: updated })
    }
  }, [workspaceId, projectId, projectPath, worktreePath, openAgentPane, tabTitle])

  return (
    <div
      className="px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
      onClick={() => openAgentPane(`__wt:${workspaceId}`, projectPath, tabTitle)}
      onContextMenu={handleContextMenu}
    >
      <div className="flex items-center gap-2">
        <svg className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0 opacity-60" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="4" cy="4" r="1.5" />
          <circle cx="12" cy="4" r="1.5" />
          <circle cx="4" cy="12" r="1.5" />
          <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
        </svg>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs text-[var(--color-text-primary)] truncate">{displayName}</span>
            {agentTemplate && (
              <span className="text-[9px] font-medium text-[var(--color-text-muted)] bg-white/5 px-1 py-0.5 flex-shrink-0">
                {agentTemplate}
              </span>
            )}
          </div>
          {branch && (
            <div className="text-[10px] text-[var(--color-text-muted)] truncate">{branch}</div>
          )}
        </div>
      </div>
    </div>
  )
}

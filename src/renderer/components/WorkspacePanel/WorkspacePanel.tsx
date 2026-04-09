import { useState, useEffect, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useActiveAgentsStore, type PaneStatus } from '@/stores/active-agents'
import { useHeartbeatScheduleStore } from '@/stores/heartbeat-schedule'
import { showContextMenu } from '@/lib/context-menu'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'

/** Convert "HH:MM" to "h:MM AM/PM" */
function fmt12h(time: string): string {
  const [hStr, mStr] = time.split(':')
  let h = parseInt(hStr, 10)
  const m = mStr ?? '00'
  const ampm = h >= 12 ? 'PM' : 'AM'
  if (h === 0) h = 12
  else if (h > 12) h -= 12
  return m === '00' ? `${h} ${ampm}` : `${h}:${m} ${ampm}`
}

/** Human-readable summary of a heartbeat schedule */
function formatScheduleSummary(mode: string, scheduleJson: string | null): string {
  if (mode === 'off' || !scheduleJson) return 'Off'
  try {
    const v = JSON.parse(scheduleJson)
    if (mode === 'hourly') {
      const secs = v.every_seconds ?? 300
      const freq = secs >= 3600 ? `${Math.round(secs / 3600)}h` : `${Math.round(secs / 60)}m`
      const start = v.start ?? '00:00'
      const end = v.end ?? '23:59'
      if (start === '00:00' && (end === '23:59' || end === '24:00')) return `Every ${freq}`
      return `Every ${freq}, ${fmt12h(start)}–${fmt12h(end)}`
    }
    // scheduled
    const freq = v.frequency ?? 'daily'
    const time = fmt12h(v.time ?? '09:00')
    if (freq === 'daily') return v.interval > 1 ? `Every ${v.interval} days, ${time}` : `Daily ${time}`
    if (freq === 'weekly') {
      const days = (v.days ?? []).map((d: string) => d.charAt(0).toUpperCase() + d.slice(1, 3)).join('/')
      return days ? `${days} ${time}` : `Weekly ${time}`
    }
    if (freq === 'monthly') return `Monthly ${time}`
    if (freq === 'yearly') return `Yearly ${time}`
    return mode
  } catch {
    return mode
  }
}

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
  manager: 'Workspace Manager',
  coordinator: 'Workspace Manager', // legacy
  pod: 'Workspace Manager', // legacy
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
  const isManagerMode = agentMode === 'manager' || agentMode === 'coordinator' || agentMode === 'pod'
  // Primary agent for any mode: manager for manager mode, first agent for custom/k2so
  const primaryAgent = useMemo(() => {
    if (isManagerMode) return agents.find((a) => a.isCoordinator) ?? null
    return agents.length > 0 ? agents[0] : null
  }, [agents, isManagerMode])
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
        {/* Mode + status + launch button */}
        <div className="flex items-center justify-between">
          <div
            className={`flex items-center gap-2 ${agentMode !== 'off' && primaryAgent ? 'cursor-pointer hover:opacity-80 transition-opacity' : ''}`}
            onClick={() => {
              if (agentMode !== 'off') {
                // Redirect to pinned system agent tab
                const tabsStore = useTabsStore.getState()
                tabsStore.activateSystemAgentTab()
              }
            }}
          >
            <span
              className="w-2.5 h-2.5 flex-shrink-0"
              style={{ backgroundColor: statusColor(projectStatus) }}
            />
            <span className="text-xs font-medium text-[var(--color-text-primary)]">
              {modeLabels[agentMode] || agentMode}
            </span>
          </div>
          {agentMode !== 'off' && (
            <button
              onClick={async () => {
                // Activate the pinned system agent tab
                const tabsStore = useTabsStore.getState()
                tabsStore.activateSystemAgentTab()

                // If the agent terminal exists but is idle, inject a checkin
                const sysTab = tabsStore.getSystemAgentTab()
                if (sysTab) {
                  const agentItem = Array.from(sysTab.paneGroups.values())[0]?.items[0]
                  if (agentItem?.type === 'agent') {
                    const terminalId = `agent-chat-${(agentItem.data as { agentName: string }).agentName}`
                    try {
                      const exists = await invoke<boolean>('terminal_exists', { id: terminalId })
                      if (exists) {
                        // Terminal exists — send checkin if idle
                        // Two-phase write: paste then Enter separately
                        await invoke('terminal_write', { id: terminalId, data: 'k2so checkin' })
                        await new Promise((r) => setTimeout(r, 150))
                        await invoke('terminal_write', { id: terminalId, data: '\r' })
                      }
                    } catch { /* terminal may not exist yet — Chat tab will handle launch */ }
                  }
                }
              }}
              className="px-2.5 py-0.5 text-[10px] font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer"
            >
              Launch
            </button>
          )}
        </div>

        {/* Work summary — inbox / delegated / review */}
        {agentMode !== 'off' && (() => {
          const totalInbox = wsInboxCount + agents.reduce((sum, a) => sum + a.inboxCount, 0)
          const totalActive = agents.reduce((sum, a) => sum + a.activeCount, 0)
          const totalDone = agents.reduce((sum, a) => sum + a.doneCount, 0)
          if (totalInbox === 0 && totalActive === 0 && totalDone === 0) return null
          return (
            <div className="flex items-center justify-evenly mt-4">
              <div className="text-center">
                <div className={`text-sm font-semibold tabular-nums ${totalInbox > 0 ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'}`}>{totalInbox}</div>
                <div className="text-[9px] text-[var(--color-text-muted)] uppercase tracking-wider">Inbox</div>
              </div>
              <div className="text-center">
                <div className={`text-sm font-semibold tabular-nums ${totalActive > 0 ? 'text-yellow-400' : 'text-[var(--color-text-muted)]'}`}>{totalActive}</div>
                <div className="text-[9px] text-[var(--color-text-muted)] uppercase tracking-wider">Active</div>
              </div>
              <div className="text-center">
                <div className={`text-sm font-semibold tabular-nums ${totalDone > 0 ? 'text-green-400' : 'text-[var(--color-text-muted)]'}`}>{totalDone}</div>
                <div className="text-[9px] text-[var(--color-text-muted)] uppercase tracking-wider">Review</div>
              </div>
            </div>
          )
        })()}

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
            <div className="border-t border-[var(--color-border)] mt-3" />
            <div className="flex items-center justify-between mt-3">
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-[var(--color-text-secondary)]">Heartbeat</span>
                {activeProject.heartbeatMode !== 'off' && (
                  <span className="text-[11px] text-red-400 animate-pulse">♥</span>
                )}
              </div>
              <button
                onClick={() => useHeartbeatScheduleStore.getState().open(activeProject.id)}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)] px-2 py-0.5 transition-colors no-drag cursor-pointer truncate max-w-[160px]"
              >
                {formatScheduleSummary(activeProject.heartbeatMode, activeProject.heartbeatSchedule)}
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

      {/* ── Connected Agents (incoming) ── */}
      <ConnectedAgentsSection projectId={activeProject.id} />

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
              No worktrees open. Click + to create one or let the manager delegate work.
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

// ── Connected Agents Section (incoming relations) ───────────────────────

interface WorkspaceRelation {
  id: string
  sourceProjectId: string
  targetProjectId: string
  relationType: string
  createdAt: string
}

function ConnectedAgentsSection({ projectId }: { projectId: string }): React.JSX.Element | null {
  const [relations, setRelations] = useState<WorkspaceRelation[]>([])
  const [loaded, setLoaded] = useState(false)
  const projects = useProjectsStore((s) => s.projects)

  useEffect(() => {
    let cancelled = false
    invoke<WorkspaceRelation[]>('workspace_relations_list_incoming', { projectId })
      .then((result) => { if (!cancelled) { setRelations(result); setLoaded(true) } })
      .catch(() => { if (!cancelled) { setRelations([]); setLoaded(true) } })
    return () => { cancelled = true }
  }, [projectId])

  // Resolve source project details
  const projectsById = useMemo(() => {
    const map = new Map<string, typeof projects[number]>()
    for (const p of projects) map.set(p.id, p)
    return map
  }, [projects])

  // Don't render anything if no incoming connections
  if (!loaded || relations.length === 0) return null

  return (
    <div className="px-3 py-2 border-b border-[var(--color-border)]">
      <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
        Connected Agents
      </span>
      <div className="mt-1.5 space-y-1">
        {relations.map((rel) => {
          const source = projectsById.get(rel.sourceProjectId)
          return (
            <div key={rel.id} className="flex items-center gap-2">
              <span
                className="w-2 h-2 flex-shrink-0 rounded-full"
                style={{ backgroundColor: source?.color || '#6b7280' }}
              />
              <span className="text-[11px] text-[var(--color-text-secondary)] truncate">
                {source?.name || 'Unknown'}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

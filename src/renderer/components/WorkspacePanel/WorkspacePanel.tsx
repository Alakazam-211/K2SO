import { useState, useEffect, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useActiveAgentsStore, type PaneStatus } from '@/stores/active-agents'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'

// ── Types ────────────────────────────────────────────────────────────────

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
  const coordinator = useMemo(() => agents.find((a) => a.isCoordinator), [agents])
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
      {/* ── Section 1: Status ── */}
      <div className="px-3 py-2 border-b border-[var(--color-border)]">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span
              className="w-2 h-2 rounded-full flex-shrink-0"
              style={{ backgroundColor: statusColor(projectStatus) }}
            />
            <span className="text-[10px] font-medium text-[var(--color-text-primary)]">
              {modeLabels[agentMode] || agentMode}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {activeProject.heartbeatEnabled === 1 && (
              <span className="text-[9px] text-red-400 animate-pulse" title="Heartbeat active">♥</span>
            )}
            {projectStatus !== 'idle' && (
              <span className="text-[9px] text-[var(--color-text-muted)]">
                {statusLabel(projectStatus)}
              </span>
            )}
          </div>
        </div>
        {wsInboxCount > 0 && (
          <div className="mt-1">
            <span className="text-[9px] text-[var(--color-accent)]">
              {wsInboxCount} item{wsInboxCount !== 1 ? 's' : ''} in inbox
            </span>
          </div>
        )}
      </div>

      {/* ── Section 2: Coordinator (hidden in Off mode) ── */}
      {agenticEnabled && agentMode !== 'off' && isCoordinatorMode && coordinator && (
        <div className="border-b border-[var(--color-border)]">
          <div className="px-3 py-1.5">
            <span className="text-[9px] font-medium text-[var(--color-accent)] uppercase tracking-wider">
              Coordinator
            </span>
          </div>
          <div
            className="px-3 py-2 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
            onClick={() => openAgentPane(coordinator.name, activeProject.path)}
          >
            <div className="flex items-center gap-2">
              <span
                className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                style={{ backgroundColor: statusColor(projectStatus) }}
              />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{coordinator.name}</span>
                  <span className="text-[9px] font-medium text-[var(--color-accent)]">COORDINATOR</span>
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] truncate">{coordinator.role}</div>
              </div>
              <div className="flex items-center gap-1 text-[9px] text-[var(--color-text-muted)] flex-shrink-0">
                {wsInboxCount > 0 && <span className="text-[var(--color-accent)]" title="Inbox">{wsInboxCount}u</span>}
              </div>
            </div>
          </div>
        </div>
      )}

      {/* ── Off mode: show inbox directly (user = coordinator) ── */}
      {agentMode === 'off' && wsInboxCount > 0 && (
        <div className="border-b border-[var(--color-border)]">
          <div
            className="px-3 py-2 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
            onClick={() => openAgentPane('__workspace__', activeProject.path)}
          >
            <div className="flex items-center gap-2">
              <svg className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
                <rect x="3" y="3" width="18" height="18" rx="2" />
                <path d="M3 9h18" />
              </svg>
              <span className="text-xs text-[var(--color-text-primary)]">Inbox</span>
              <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-accent)]/10 text-[var(--color-accent)]">
                {wsInboxCount}
              </span>
            </div>
          </div>
        </div>
      )}

      {/* ── Section 3: Worktrees ── */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
          Worktrees
          {worktrees.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-white/5 text-[var(--color-text-muted)]">
              {worktrees.length}
            </span>
          )}
        </span>
        {activeProject.worktreeMode === 1 && (
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
        )}
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
              {activeProject.worktreeMode === 1
                ? 'No worktrees open. Create one from the sidebar or let the coordinator delegate work.'
                : 'Enable worktrees in workspace settings to use parallel branches.'}
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
  displayName,
  branch,
  agentTemplate,
}: {
  workspaceId: string
  projectId: string
  projectPath: string
  displayName: string
  branch: string | null
  agentTemplate?: string
}): React.JSX.Element {
  const openAgentPane = useTabsStore((s) => s.openAgentPane)
  const tabTitle = displayName || branch || 'Worktree'

  return (
    <div
      className="px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
      onClick={() => openAgentPane(`__wt:${workspaceId}`, projectPath, tabTitle)}
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

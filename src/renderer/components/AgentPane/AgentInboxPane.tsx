import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'

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
  isCoordinator: boolean
  agentType: string
}

interface AgentInboxPaneProps {
  agentName: string
  projectPath: string
}

// ── Helpers ─────────────────────────────────────────────────────────────

const priorityBadge = (p: string): string => {
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
  agentDir?: string
  onOpenFile: (path: string) => void
}): React.JSX.Element {
  const resolvePath = (item: WorkItem): string => {
    if (item.assignedBy && item.assignedBy !== 'user' && item.assignedBy !== 'external' && item.assignedBy !== 'delegated') {
      return `${projectPath}/.k2so/agents/${item.assignedBy}/work/${item.folder}/${item.filename}`
    }
    if (agentDir) {
      return `${agentDir}/work/${item.folder}/${item.filename}`
    }
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

// ── Main Component ──────────────────────────────────────────────────────

/**
 * Inbox pinned tab — shows the work queue for an agent or the
 * workspace-level board (when agentName === '__workspace__').
 *
 * Replaces the "Work" sub-tab from the pre-0.36.0 single AgentPane.
 * Sibling tab is `AgentChatPane`; both are pinned by `tabs.ts`.
 */
export function AgentInboxPane({ agentName, projectPath }: AgentInboxPaneProps): React.JSX.Element {
  const isWorkspaceBoard = agentName === '__workspace__'

  const [profile, setProfile] = useState<AgentProfile | null>(null)
  const [workItems, setWorkItems] = useState<WorkItem[]>([])
  const [wsInboxItems, setWsInboxItems] = useState<WorkItem[]>([])
  const [allAgentWork, setAllAgentWork] = useState<WorkItem[]>([])

  const agentDir = `${projectPath}/.k2so/agents/${agentName}`

  const fetchProfile = useCallback(async () => {
    if (isWorkspaceBoard) return
    try {
      const result = await invoke<string | { content: string }>('k2so_agents_get_profile', { projectPath, agentName })
      const raw = typeof result === 'string' ? result : (result.content || '')
      const fmMatch = raw.match(/^---\n([\s\S]*?)\n---/)
      let isCoordinator = false
      let agentType = 'agent-template'
      if (fmMatch) {
        const fm = fmMatch[1]
        isCoordinator = fm.match(/^pod_leader:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^coordinator:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^manager:\s*(.+)$/m)?.[1]?.trim() === 'true'
        const rawType = fm.match(/^type:\s*(.+)$/m)?.[1]?.trim() || 'agent-template'
        agentType = rawType === 'pod-leader' || rawType === 'manager'
          ? 'coordinator'
          : rawType === 'pod-member' ? 'agent-template' : rawType
      }
      setProfile({ isCoordinator, agentType })
    } catch {
      setProfile(null)
    }
  }, [projectPath, agentName, isWorkspaceBoard])

  const isManager = profile?.isCoordinator
    || profile?.agentType === 'coordinator'
    || profile?.agentType === 'manager'

  const fetchWork = useCallback(async () => {
    if (isWorkspaceBoard || isManager) {
      try {
        const wsItems = await invoke<WorkItem[]>('k2so_agents_workspace_inbox_list', { projectPath })
        setWsInboxItems(wsItems)
      } catch { setWsInboxItems([]) }
      try {
        const agents = await invoke<{ name: string }[]>('k2so_agents_list', { projectPath })
        const all: WorkItem[] = []
        for (const agent of agents) {
          if (agent.name === agentName) continue
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
    fetchProfile()
    fetchWork()
    const interval = setInterval(fetchWork, 10_000)
    return () => clearInterval(interval)
  }, [fetchProfile, fetchWork])

  const openFile = (filePath: string): void => useTabsStore.getState().openFileAsTab(filePath)

  // Single-agent columns
  const inbox = workItems.filter((w) => w.folder === 'inbox')
  const active = workItems.filter((w) => w.folder === 'active')
  const done = workItems.filter((w) => w.folder === 'done')

  // Workspace / manager columns
  const wsUnassigned = wsInboxItems
  const wsInProgress = allAgentWork.filter((w) => w.folder === 'inbox' || w.folder === 'active')
  const wsReview = allAgentWork.filter((w) => w.folder === 'done')

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {isWorkspaceBoard ? 'Work Board' : agentName}
        </span>
        {profile?.isCoordinator && (
          <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 flex-shrink-0">
            MANAGER
          </span>
        )}
      </div>

      <div className="flex-1 overflow-hidden min-h-0 relative">
        {isWorkspaceBoard ? (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto">
            <KanbanColumn title="Unassigned" items={wsUnassigned} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="In Progress" items={wsInProgress} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={wsReview} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        ) : isManager ? (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
            <KanbanColumn title="Inbox" items={wsUnassigned} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Delegated" items={wsInProgress} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={wsReview} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        ) : (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
            <KanbanColumn title="Inbox" items={inbox} color="text-[var(--color-accent)]" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
            <KanbanColumn title="Active" items={active} color="text-yellow-400" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
            <KanbanColumn title="Done" items={done} color="text-green-400" projectPath={projectPath} agentDir={agentDir} onOpenFile={openFile} />
          </div>
        )}
      </div>
    </div>
  )
}

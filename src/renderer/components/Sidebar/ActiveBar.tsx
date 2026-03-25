import { useMemo, useCallback, useState, useEffect } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { invoke } from '@tauri-apps/api/core'
import { showContextMenu } from '@/lib/context-menu'
import ProjectAvatar from './ProjectAvatar'
import { KeyCombo } from '@/components/KeySymbol'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const TWENTY_FOUR_HOURS = 24 * 60 * 60

/**
 * In-memory set of project IDs that have been in the Active bar.
 * This prevents projects from flickering out during workspace switches
 * when background/DB state is temporarily inconsistent.
 * Only cleared by explicit dismiss or 24h expiry.
 */
const _activeBarMemory = new Set<string>()

/** Compute which projects appear in the Active Bar */
function useActiveBarItems(): ProjectWithWorkspaces[] {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const backgroundWorkspaces = useTabsStore((s) => s.backgroundWorkspaces)
  const hasActiveAgents = useActiveAgentsStore((s) => s.hasActiveAgents())
  const paneStatuses = useActiveAgentsStore((s) => s.paneStatuses)

  // Refresh the 24h check periodically (every 60s)
  const [tick, setTick] = useState(0)
  useEffect(() => {
    const interval = setInterval(() => setTick((t) => t + 1), 60000)
    return () => clearInterval(interval)
  }, [])

  return useMemo(() => {
    const now = Math.floor(Date.now() / 1000)

    // Check if any pane has a non-idle hook status (agent was recently active)
    const hasHookActivity = paneStatuses.size > 0 && Array.from(paneStatuses.values()).some(
      (s) => s === 'working' || s === 'permission' || s === 'review'
    )

    const result = projects.filter((p) => {
      // Skip pinned projects — they're always visible at the top
      if (p.pinned) return false

      // 1. Manually active — always included
      if (p.manuallyActive) return true

      // 2. Recently interacted (within 24h, set when agent message sent)
      if (p.lastInteractionAt && (now - p.lastInteractionAt) < TWENTY_FOUR_HOURS) return true

      // 3. Is the active project with running agents (hook or poll detected)
      if (p.id === activeProjectId && (hasActiveAgents || hasHookActivity)) return true

      // 4. Has background workspaces (stashed terminals)
      const hasBackground = Object.keys(backgroundWorkspaces).some(
        (key) => key.startsWith(`${p.id}:`)
      )
      if (hasBackground) return true

      // 5. Was previously in the active bar (memory — prevents flicker during switches)
      if (_activeBarMemory.has(p.id)) return true

      return false
    })

    // Remember items that entered the active bar
    for (const p of result) _activeBarMemory.add(p.id)

    return result
  }, [projects, activeProjectId, backgroundWorkspaces, hasActiveAgents, paneStatuses, tick])
}

function ActiveBarItem({
  project,
  index,
  isCurrentProject,
  onClick,
  onContextMenu,
}: {
  project: ProjectWithWorkspaces
  index: number
  isCurrentProject: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
}): React.JSX.Element {
  const shortcutNum = index < 9 ? index + 1 : index === 9 ? 0 : null
  const projectAgentStatus = useActiveAgentsStore((s) => s.getProjectStatus(project.id))
  const isAgentWorking = projectAgentStatus === 'working' || projectAgentStatus === 'permission'

  return (
    <button
      onClick={onClick}
      onContextMenu={onContextMenu}
      className={`no-drag w-full flex items-center gap-2 px-2 py-1 text-left transition-colors cursor-pointer select-none ${
        isCurrentProject
          ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
      }`}
    >
      <ProjectAvatar
        projectPath={project.path}
        projectName={project.name}
        projectColor={project.color}
        projectId={project.id}
        iconUrl={project.iconUrl}
        size={18}
      />
      <span className="text-[11px] truncate flex-1">{project.name}</span>
      {isAgentWorking && (
        <span className={`text-[11px] font-mono flex-shrink-0 ${
          projectAgentStatus === 'permission' ? 'text-red-400' : 'text-[var(--color-text-muted)]'
        }`}>
          <span className="braille-spinner" />
        </span>
      )}
      {shortcutNum !== null && (
        <span className="text-[10px] font-mono text-[var(--color-text-muted)] flex-shrink-0 tabular-nums">
          {shortcutNum}
        </span>
      )}
    </button>
  )
}

export default function ActiveBar(): React.JSX.Element | null {
  const items = useActiveBarItems()
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const setManuallyActive = useProjectsStore((s) => s.setManuallyActive)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const setActiveFocusGroup = useFocusGroupsStore((s) => s.setActiveFocusGroup)
  const agentMap = useActiveAgentsStore((s) => s.agents)
  const agentStatus = useActiveAgentsStore((s) => s.getAggregateStatus())
  const shortcutLayout = useTerminalSettingsStore((s) => s.shortcutLayout)

  const handleClick = useCallback((project: ProjectWithWorkspaces) => {
    const firstWs = project.workspaces[0]
    if (!firstWs) return

    // Switch focus group if needed
    if (focusGroupsEnabled && project.focusGroupId) {
      setActiveFocusGroup(project.focusGroupId)
    }

    setActiveWorkspace(project.id, firstWs.id)
  }, [focusGroupsEnabled, setActiveFocusGroup, setActiveWorkspace])

  const handleContextMenu = useCallback(async (e: React.MouseEvent, project: ProjectWithWorkspaces) => {
    e.preventDefault()

    // Check if project has running agents (only possible if it's the active project)
    const hasRunningAgent = project.id === activeProjectId && Array.from(agentMap.values()).some(
      (a) => a.status === 'active'
    )

    const menuItems: Array<{ id: string; label: string; type?: string }> = []

    if (project.manuallyActive) {
      menuItems.push({ id: 'remove-permanent', label: 'Remove from Active Bar' })
    } else {
      menuItems.push({ id: 'add-active', label: 'Keep in Active Bar' })
    }

    menuItems.push({ id: 'sep', label: '', type: 'separator' })

    if (hasRunningAgent) {
      menuItems.push({ id: 'dismiss-blocked', label: 'Dismiss (agent running)' })
    } else {
      menuItems.push({ id: 'dismiss', label: 'Dismiss' })
    }

    const clickedId = await showContextMenu(menuItems)
    if (clickedId === 'remove-permanent') {
      _activeBarMemory.delete(project.id)
      await setManuallyActive(project.id, false)
    } else if (clickedId === 'add-active') {
      await setManuallyActive(project.id, true)
    } else if (clickedId === 'dismiss' && !hasRunningAgent) {
      // Clear from memory, DB, and local state
      _activeBarMemory.delete(project.id)
      await invoke('projects_update', { id: project.id, manuallyActive: 0 })
      await invoke('projects_touch_interaction_clear', { id: project.id }).catch(() => {})
      await useProjectsStore.getState().fetchProjects()
    }
  }, [activeProjectId, agentMap, setManuallyActive])

  const [collapsed, setCollapsed] = useState(false)

  if (items.length === 0) return null

  return (
    <div className="border-t border-[var(--color-border)] flex flex-col">
      <button
        className="no-drag w-full flex items-center gap-1.5 px-3 pt-2 pb-1 text-left cursor-pointer hover:bg-white/[0.02] transition-colors"
        onClick={() => setCollapsed((prev) => !prev)}
      >
        <span className="text-[10px] font-semibold tracking-wider text-[var(--color-text-muted)] uppercase">
          Active
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono">
          {items.length}
        </span>
        <span className="text-[9px] font-mono text-[var(--color-text-muted)] opacity-50">
          <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌘ 1-9' : '⇧⌘ 1-9'} />
        </span>
        <span className="flex-1" />
        <svg
          className="w-2.5 h-2.5 text-[var(--color-text-muted)] flex-shrink-0"
          style={{ transition: 'transform 0.2s ease', transform: collapsed ? 'rotate(0deg)' : 'rotate(90deg)' }}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
      </button>
      <div
        style={{
          overflow: 'hidden',
          maxHeight: collapsed ? 0 : 200,
          transition: 'max-height 0.2s ease',
        }}
      >
        <div className="px-1 pb-1">
        {items.map((project, index) => (
          <ActiveBarItem
            key={project.id}
            project={project}
            index={index}
            isCurrentProject={project.id === activeProjectId}
            onClick={() => handleClick(project)}
            onContextMenu={(e) => handleContextMenu(e, project)}
          />
        ))}
        </div>
      </div>
    </div>
  )
}

/** Non-hook version for use by keyboard shortcuts — same logic as useActiveBarItems */
export function getActiveBarItems(): ProjectWithWorkspaces[] {
  const projects = useProjectsStore.getState().projects
  const activeProjectId = useProjectsStore.getState().activeProjectId
  const backgroundWorkspaces = useTabsStore.getState().backgroundWorkspaces
  const hasActiveAgents = useActiveAgentsStore.getState().hasActiveAgents()
  const paneStatuses = useActiveAgentsStore.getState().paneStatuses
  const now = Math.floor(Date.now() / 1000)

  const hasHookActivity = paneStatuses.size > 0 && Array.from(paneStatuses.values()).some(
    (s) => s === 'working' || s === 'permission' || s === 'review'
  )

  return projects.filter((p) => {
    if (p.pinned) return false
    if (p.manuallyActive) return true
    if (p.lastInteractionAt && (now - p.lastInteractionAt) < TWENTY_FOUR_HOURS) return true
    if (p.id === activeProjectId && (hasActiveAgents || hasHookActivity)) return true
    if (Object.keys(backgroundWorkspaces).some((k) => k.startsWith(`${p.id}:`))) return true
    if (_activeBarMemory.has(p.id)) return true
    return false
  })
}

/** Export for use by keyboard shortcuts */
export { useActiveBarItems }

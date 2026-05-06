import { useMemo, useCallback, useState, useEffect, useRef } from 'react'
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
 * In-memory map of project IDs → unix-second timestamp at which they
 * were first observed in the Active bar. Prevents flicker during
 * workspace switches when background/DB state is temporarily
 * inconsistent (rule 5 in `useActiveBarItems`).
 *
 * Each entry has a 24h TTL — older entries are pruned by the same
 * filter pass that reads them, plus a periodic pass on the 60s
 * `tick`. Without the TTL, rule 5 would override every other
 * dismiss path (explicit dismiss, 24h interaction expiry on rule 2)
 * because once a project enters memory it never leaves.
 *
 * Cleared by:
 *   - Explicit dismiss (`Dismiss` context-menu item) → immediate `delete`.
 *   - 24h elapsed since the entry was added → pruned on next read /
 *     periodic tick.
 *   - "Remove from Active Bar" (manual-active toggle off) →
 *     immediate `delete`.
 */
const _activeBarMemory = new Map<string, number>()

function pruneExpiredActiveBarMemory(now: number): void {
  for (const [id, addedAt] of _activeBarMemory) {
    if (now - addedAt >= TWENTY_FOUR_HOURS) {
      _activeBarMemory.delete(id)
    }
  }
}

/**
 * Set of project IDs the user has explicitly dismissed in this
 * session. Used to override the auto-include rules (currently-active
 * workspace, has-background-workspaces, in-memory) so an explicit
 * Dismiss action always takes effect immediately — even when the
 * dismissed project happens to be the workspace the user is
 * currently viewing.
 *
 * Without this, dismissing the active workspace was a no-op
 * visually: DB cleared, _activeBarMemory cleared, but rule 3
 * (`p.id === activeProjectId`) re-added the project on the next
 * render. The user only saw the dismiss after reloading the
 * Tauri page (which reset the in-memory active project id).
 *
 * Cleared by:
 *   - User navigates to a different workspace (the dismiss is
 *     "complete" once they've moved on; coming back re-engages
 *     normal rules).
 *   - Manual re-add ("Keep in Active Bar" sets manuallyActive=1).
 *   - The TTL matches `_activeBarMemory` (24h) for safety, even
 *     though navigation usually clears it well before then.
 */
const _dismissedProjects = new Map<string, number>()

function pruneExpiredDismissedProjects(now: number): void {
  for (const [id, dismissedAt] of _dismissedProjects) {
    if (now - dismissedAt >= TWENTY_FOUR_HOURS) {
      _dismissedProjects.delete(id)
    }
  }
}

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

  // Track the previous activeProjectId so we can detect "navigated
  // away from a dismissed project" → clear that project's dismissed
  // bit so a future return re-engages normal rules. Without this
  // ref, simply checking `if (activeProjectId)` on each render would
  // clear the dismissed entry on the very next render after dismiss
  // (the dismissed project IS still active right then), defeating
  // the dismiss visually.
  const prevActiveProjectIdRef = useRef<string | null>(null)
  useEffect(() => {
    const prev = prevActiveProjectIdRef.current
    if (prev && prev !== activeProjectId && _dismissedProjects.has(prev)) {
      _dismissedProjects.delete(prev)
    }
    prevActiveProjectIdRef.current = activeProjectId
  }, [activeProjectId])

  return useMemo(() => {
    const now = Math.floor(Date.now() / 1000)

    // Prune both TTL maps before reading. Guarantees explicit-dismiss
    // and 24h-auto-dismiss invariants hold without a separate
    // background sweep.
    pruneExpiredActiveBarMemory(now)
    pruneExpiredDismissedProjects(now)

    // (Dismissed-bit cleanup on activeProjectId change is handled in
    // the prevActiveProjectIdRef effect above. Doing it inline here
    // would clear the dismiss on the very next render after the
    // user dismissed the currently-active project.)

    // Check if any pane has a non-idle hook status (agent was recently active)
    const hasHookActivity = paneStatuses.size > 0 && Array.from(paneStatuses.values()).some(
      (s) => s === 'working' || s === 'permission' || s === 'review'
    )

    const result = projects.filter((p) => {
      // Skip pinned projects — they're always visible at the top
      if (p.pinned) return false

      // Skip single-agent workspaces (K2SO Agent, Custom Agent) — shown in agents section
      // Coordinator workspaces with worktrees should still appear in the active bar
      if (p.agentMode === 'agent' || p.agentMode === 'custom') return false

      // 1. Manually active — always included (explicit user signal,
      // wins over a stale dismiss).
      if (p.manuallyActive) return true

      // 2. Recently interacted (within 24h, set when agent message sent).
      // Also an explicit user signal — wins over dismiss.
      if (p.lastInteractionAt && (now - p.lastInteractionAt) < TWENTY_FOUR_HOURS) return true

      // Explicit dismiss in this session overrides the auto-include
      // rules below. Without this gate, dismissing the workspace the
      // user is currently viewing was a no-op visually — rule 3
      // re-added the project before the next render painted. The
      // dismissed-bit clears when the user navigates away (above)
      // or after 24h.
      if (_dismissedProjects.has(p.id)) return false

      // 3. Is the currently-active workspace. The user is looking at
      // it right now; surfacing it in Active gives them an obvious
      // landing spot when they navigate away and come back. Pre-A
      // this rule additionally required `hasActiveAgents ||
      // hasHookActivity`, which meant v2 tabs whose agent-detection
      // hadn't lit up yet would never enter the bar — and once the
      // user navigated away they'd lose any "I was just here" trail.
      // Always-include-when-active matches the legacy Tauri behavior
      // users expect from iTerm-style tabbed shells.
      if (p.id === activeProjectId) return true

      // 4. Has background workspaces (stashed terminals)
      const hasBackground = Object.keys(backgroundWorkspaces).some(
        (key) => key.startsWith(`${p.id}:`)
      )
      if (hasBackground) return true

      // 5. Was previously in the active bar within the last 24h
      // (memory — prevents flicker during workspace switches). The
      // map's TTL is honored by the prune above; entries older than
      // 24h are gone before this read fires.
      if (_activeBarMemory.has(p.id)) return true

      return false
    })

    // Remember items that just entered the active bar. Stamp `now`
    // only on first add — re-adding an existing entry doesn't reset
    // its 24h timer (otherwise an always-on workspace could never
    // fall out of memory by being constantly re-observed). The 24h
    // is from FIRST appearance, not most-recent-render.
    for (const p of result) {
      if (!_activeBarMemory.has(p.id)) {
        _activeBarMemory.set(p.id, now)
      }
    }

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
      _dismissedProjects.delete(project.id)
      await setManuallyActive(project.id, false)
    } else if (clickedId === 'add-active') {
      // "Keep in Active Bar" is an explicit re-add — clears any
      // stale dismiss state so the manual flag wins immediately.
      _dismissedProjects.delete(project.id)
      await setManuallyActive(project.id, true)
    } else if (clickedId === 'dismiss' && !hasRunningAgent) {
      // Clear from memory, DB, background workspaces, and local state.
      // Also stamp the dismissed-bit so rules 3/4/5 don't re-add the
      // project on the next render — this is the visible-immediately
      // fix for "I dismissed the workspace I'm currently viewing
      // and nothing changed until reload."
      const now = Math.floor(Date.now() / 1000)
      _activeBarMemory.delete(project.id)
      _dismissedProjects.set(project.id, now)
      await invoke('projects_update', { id: project.id, manuallyActive: 0 })
      await invoke('projects_touch_interaction_clear', { id: project.id }).catch((e) => console.warn('[active-bar]', e))
      // Clear background workspaces for this project (stashed terminals keep it visible)
      const tabsStore = useTabsStore.getState()
      for (const key of Object.keys(tabsStore.backgroundWorkspaces)) {
        if (key.startsWith(`${project.id}:`)) {
          tabsStore.clearBackgroundWorkspace(key)
        }
      }
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
          <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌘ 1-9' : '⌥⌘ 1-9'} />
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
  const now = Math.floor(Date.now() / 1000)

  // Honor the same 24h TTLs as the hook version. Without these
  // prunes, stale entries would override the dismiss path here too.
  pruneExpiredActiveBarMemory(now)
  pruneExpiredDismissedProjects(now)

  return projects.filter((p) => {
    if (p.pinned) return false
    // Only exclude single-agent workspaces (shown in agents section), not pods
    if (p.agentMode === 'agent' || p.agentMode === 'custom') return false
    if (p.manuallyActive) return true
    if (p.lastInteractionAt && (now - p.lastInteractionAt) < TWENTY_FOUR_HOURS) return true
    if (_dismissedProjects.has(p.id)) return false
    if (p.id === activeProjectId) return true
    if (Object.keys(backgroundWorkspaces).some((k) => k.startsWith(`${p.id}:`))) return true
    if (_activeBarMemory.has(p.id)) return true
    return false
  })
}

/** Export for use by keyboard shortcuts */
export { useActiveBarItems }

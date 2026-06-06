import { useMemo, useCallback, useState, useEffect, useRef, Fragment } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useActiveAgentsStore, projectHasLiveSession } from '@/stores/active-agents'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useSettingsStore, clampActiveWindowHours } from '@/stores/settings'
import { emit } from '@tauri-apps/api/event'
// Plan B — project mutations are host-aware daemon data: route through
// the `/cli/projects/*` HTTP layer. `projects_update` re-emits
// `sync:projects` for cross-window refresh; `touch-interaction-clear`
// emitted no sync.
import { daemonCliPost } from '@/lib/daemon-cli'
// #625 — clear the local-ID-keyed Active Bar memory on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'
import { showContextMenu } from '@/lib/context-menu'
import ProjectAvatar from './ProjectAvatar'
import { KeyCombo } from '@/components/KeySymbol'
import { IconAutonomous } from '@/components/icons/IconAutonomous'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const TWENTY_FOUR_HOURS = 24 * 60 * 60

/**
 * P1.C — Active-Bar rule 2 predicate, extracted so it's unit-testable and
 * reads the user-configured window. `lastInteractionAt` and `nowSecs` are
 * unix SECONDS (matching the rest of this module); `windowHours` is the
 * configured tenure (default 24, min 1). Returns true while the workspace
 * is still inside its post-interaction Active window.
 */
export function isWithinActiveWindow(
  lastInteractionAt: number | null | undefined,
  nowSecs: number,
  windowHours: number,
): boolean {
  if (!lastInteractionAt) return false
  const windowSecs = clampActiveWindowHours(windowHours) * 60 * 60
  return nowSecs - lastInteractionAt < windowSecs
}

/**
 * EKG (heartbeat) badge predicate for the Active-bar indicator.
 *
 * CONFIG-FLAG semantics (user decision 2026-06-06): the EKG icon means
 * "this workspace has at least one enabled heartbeat" — i.e. it CAN run
 * on its own — NOT "it is self-driving right this second." It's a static
 * capability flag, persistent for as long as a heartbeat is enabled,
 * shown on any Active item. It deliberately does NOT depend on the Active
 * window or on whether a heartbeat recently fired, and it COEXISTS with
 * the braille spinner (EKG = configured to self-drive; spinner = working
 * now). `heartbeatEnabled` is the live aggregate from projects/list (any
 * enabled, non-archived heartbeat).
 *
 * Pure + exported so it's unit-testable without mounting the component.
 */
export function hasEnabledHeartbeat(heartbeatEnabled: number): boolean {
  return heartbeatEnabled !== 0
}

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

/**
 * #625 — clear the Active Bar's local-ID-keyed session memory.
 *
 * `_activeBarMemory` and `_dismissedProjects` are module-level Maps keyed
 * by LOCAL project IDs. As module singletons they survive the
 * `<App key={hostKey}>` remount on a host switch, so after connecting to a
 * REMOTE daemon rule 5 (`_activeBarMemory.has(p.id)`) and the dismiss gate
 * would still reference the previous host's project IDs. Reset both on a
 * real host CHANGE. Exported so a focused test can drive it directly.
 */
export function __resetActiveBarMemoryForHostSwitch(): void {
  _activeBarMemory.clear()
  _dismissedProjects.clear()
}

// Fires only on a real active-host change (never the initial 'local'),
// AFTER `activeHost` has flipped. connect-host.ts imports nothing
// app-side, so this subscription introduces no import cycle.
onActiveHostChange(() => {
  __resetActiveBarMemoryForHostSwitch()
})

/** Test-only inspection of the Active Bar memory maps (#625). */
export function __activeBarMemoryForTests(): {
  memory: Map<string, number>
  dismissed: Map<string, number>
} {
  return { memory: _activeBarMemory, dismissed: _dismissedProjects }
}

/** Compute which projects appear in the Active Bar */
function useActiveBarItems(): ProjectWithWorkspaces[] {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const backgroundWorkspaces = useTabsStore((s) => s.backgroundWorkspaces)
  const hasActiveAgents = useActiveAgentsStore((s) => s.hasActiveAgents())
  const paneStatuses = useActiveAgentsStore((s) => s.paneStatuses)
  const activeWindowHours = useSettingsStore((s) => s.activeWindowHours)

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

  // P2 — age-out sweep. Piggyback the existing 60s `tick`: any project
  // that has aged out of the Active window (and isn't pinned / the
  // foreground / heartbeat-managed) has its pinned-Chat PTY reaped via
  // the #657 dismiss path, freeing RAM. The decision uses the SAME
  // `isWithinActiveWindow` predicate the bar membership uses (single
  // source of truth); tabs.ts owns the foreground gate and the
  // 15s-grace scheduling. Renderer-driven because the daemon has no
  // `lastInteractionAt` access.
  //
  // TWO candidate sources, run together so nothing aged-out escapes:
  //
  //   1. `sweepAgedOutWorkspaceChats` over the projects store — fast,
  //      reaps workspaces whose chat PTY is stashed in THIS renderer's
  //      `backgroundWorkspaces` snapshot.
  //
  //   2. `sweepAgedOutWorkspaceChatsFromDaemon` over the daemon's live
  //      PTY list — reaches workspaces that aged out WHILE HIDDEN or
  //      whose chat PTY survives from a PRIOR app session (never opened
  //      in this renderer, so source 1 + the old Active-bar-fed sweep
  //      could never see them — an aged-out workspace is absent from the
  //      Active bar by construction). This is the LIVE-observed leak: 6
  //      aged-out workspaces with live daemon chat PTYs that the
  //      Active-bar-fed sweep never received as candidates.
  useEffect(() => {
    const now = Math.floor(Date.now() / 1000)
    const candidates = projects.map((p) => ({
      projectId: p.id,
      projectPath: p.path,
      isAged: !isWithinActiveWindow(p.lastInteractionAt, now, activeWindowHours),
      manuallyActive: p.manuallyActive !== 0,
      heartbeatEnabled: p.heartbeatEnabled !== 0,
    }))
    const tabsStore = useTabsStore.getState()
    tabsStore.sweepAgedOutWorkspaceChats(candidates)

    // Build the projectId-keyed verdict map for the daemon-driven sweep
    // from the same per-project signals.
    const metaByProjectId: Record<string, import('@/stores/tabs').AgeOutProjectMeta> = {}
    for (const c of candidates) {
      metaByProjectId[c.projectId] = {
        projectPath: c.projectPath,
        isAged: c.isAged,
        manuallyActive: c.manuallyActive,
        heartbeatEnabled: c.heartbeatEnabled,
      }
    }
    void tabsStore.sweepAgedOutWorkspaceChatsFromDaemon(metaByProjectId)
    // `tick` (the 60s interval) is the cadence driver; projects /
    // activeWindowHours re-run it immediately on a relevant change.
  }, [projects, activeWindowHours, tick])

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
      // 0.37.13 — pinned + agent-mode workspaces are no longer
      // filtered out of Active. They show in both their own
      // section (Agents & Pinned at the top) AND in Active if
      // they meet the activity criteria below. This makes the
      // 1-0 keyboard shortcuts work on the actual workspaces the
      // user is using right now, not just the unpinned ones.

      // 1. Manually active — always included (explicit user signal,
      // wins over a stale dismiss).
      if (p.manuallyActive) return true

      // 2. Recently interacted (within the configurable Active window,
      // set when agent message sent / workspace activated). Also an
      // explicit user signal — wins over dismiss.
      if (isWithinActiveWindow(p.lastInteractionAt, now, activeWindowHours)) return true

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

    return sortPinnedFirst(result)
  }, [projects, activeProjectId, backgroundWorkspaces, hasActiveAgents, paneStatuses, tick, activeWindowHours])
}

/** Stable partition: manually-pinned (Active-pinned) workspaces float to the
 *  top, everything else keeps its existing order. The renderer draws a single
 *  separator at the pinned↔rest boundary so the pinned group reads apart
 *  without needing a per-item icon. Applied to BOTH the hook and the
 *  keyboard-shortcut variant so Cmd/⌥⌘ 1-9 follow the visible order. */
function sortPinnedFirst(items: ProjectWithWorkspaces[]): ProjectWithWorkspaces[] {
  const pinned = items.filter((p) => p.manuallyActive)
  if (pinned.length === 0 || pinned.length === items.length) return items
  return [...pinned, ...items.filter((p) => !p.manuallyActive)]
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
  // Green "live session" dot — the workspace currently holds a live PTY
  // (i.e. it's consuming RAM and is reapable on dismiss/age-out). Selector
  // returns a boolean so the item only re-renders when its liveness flips.
  const hasLiveSession = useActiveAgentsStore((s) => projectHasLiveSession(s.liveSessionCwds, project.path))

  // EKG (heartbeat) badge — config-flag semantics: shown whenever this
  // workspace has ≥1 enabled heartbeat, i.e. it CAN self-drive. Persistent
  // (not gated on recency or on a recent fire) and coexists with the
  // braille spinner. Distinct glyph (EKG pulse) reads as "has autonomous
  // heartbeats" vs the spinner's "working now".
  const showEkg = hasEnabledHeartbeat(project.heartbeatEnabled)

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
      {showEkg && (
        <span
          className="flex-shrink-0 text-[var(--color-text-muted)] opacity-80"
          title="Has an enabled heartbeat — this workspace can run on its own"
        >
          <IconAutonomous className="w-3.5 h-3.5" />
        </span>
      )}
      {/* Session-liveness square, RIGHT of the EKG — always shown: green when
          ≥1 live session (consuming RAM, reapable), grey when none alive. */}
      <span
        className={`flex-shrink-0 w-1.5 h-1.5 rounded-[1px] ${
          hasLiveSession ? 'bg-green-500' : 'bg-white/20'
        }`}
        title={hasLiveSession ? 'Live session running' : 'No live session'}
      />
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
      // #657 — also cancel any pending dismiss-reap of the chat PTY so
      // a re-add inside the 15s grace keeps the warm session.
      _dismissedProjects.delete(project.id)
      useTabsStore.getState().cancelWorkspaceChatReap(project.id)
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
      await daemonCliPost('projects/update', { id: project.id, manuallyActive: 0 })
      void emit('sync:projects').catch(() => {})
      await daemonCliPost('projects/touch-interaction-clear', { id: project.id }).catch((e) => console.warn('[active-bar]', e))
      // Clear background workspaces for this project (stashed terminals keep it visible)
      const tabsStore = useTabsStore.getState()
      for (const key of Object.keys(tabsStore.backgroundWorkspaces)) {
        if (key.startsWith(`${project.id}:`)) {
          tabsStore.clearBackgroundWorkspace(key)
        }
      }
      // #657 — schedule the pinned Chat (agent) PTY to be reaped after
      // a 15s grace delay so its memory is freed. clearBackgroundWorkspace
      // above only kills terminal items; the chat is an agent item and
      // survives. The schedule action skips entirely if this project is
      // still the foreground (rule 2) and re-checks at fire time; a
      // re-open/re-activate within the window cancels it. On return the
      // saved session lazily resumes via `claude --resume`.
      tabsStore.scheduleWorkspaceChatReap(project.id, project.path)
      await useProjectsStore.getState().fetchProjects()
    }
  }, [activeProjectId, agentMap, setManuallyActive])

  const [collapsed, setCollapsed] = useState(false)

  // Boundary index for the pinned↔rest separator. Items are already sorted
  // pinned-first by sortPinnedFirst, so this is just the pinned count.
  const pinnedCount = items.filter((p) => p.manuallyActive).length

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
          // 0.37.13 — 320px fits all 10 shortcut-bound items (1-9 + 0)
          // at ~28-30px per row. Anything beyond rolls off the end.
          maxHeight: collapsed ? 0 : 320,
          transition: 'max-height 0.2s ease',
        }}
      >
        <div className="px-1 pb-1">
        {items.map((project, index) => (
          <Fragment key={project.id}>
            {/* Single separator at the pinned↔rest boundary (pinned floated
                to top). Only renders when both groups are non-empty. */}
            {index === pinnedCount && pinnedCount > 0 && (
              <div className="mx-2 my-1 border-t border-[var(--color-border)]" />
            )}
            <ActiveBarItem
              project={project}
              index={index}
              isCurrentProject={project.id === activeProjectId}
              onClick={() => handleClick(project)}
              onContextMenu={(e) => handleContextMenu(e, project)}
            />
          </Fragment>
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
  const activeWindowHours = useSettingsStore.getState().activeWindowHours
  const now = Math.floor(Date.now() / 1000)

  // Honor the same 24h TTLs as the hook version. Without these
  // prunes, stale entries would override the dismiss path here too.
  pruneExpiredActiveBarMemory(now)
  pruneExpiredDismissedProjects(now)

  return sortPinnedFirst(projects.filter((p) => {
    // 0.37.13 — keep pinned + agent-mode workspaces eligible for
    // Active. Same rationale as the hook above: 1-0 shortcuts should
    // work on the workspaces the user is actually using.
    if (p.manuallyActive) return true
    if (isWithinActiveWindow(p.lastInteractionAt, now, activeWindowHours)) return true
    if (_dismissedProjects.has(p.id)) return false
    if (p.id === activeProjectId) return true
    if (Object.keys(backgroundWorkspaces).some((k) => k.startsWith(`${p.id}:`))) return true
    if (_activeBarMemory.has(p.id)) return true
    return false
  }))
}

/** Export for use by keyboard shortcuts */
export { useActiveBarItems }

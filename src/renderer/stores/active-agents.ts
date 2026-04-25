import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore, type TerminalItemData } from './tabs'
import { useToastStore } from './toast'
import { useProjectsStore } from './projects'
import { usePresetsStore, parseCommand } from './presets'
import { useSettingsStore } from './settings'
import { KNOWN_AGENT_COMMANDS, AGENT_IDLE_THRESHOLD_MS } from '@shared/constants'

export type PaneStatus = 'idle' | 'working' | 'permission' | 'review'

export interface ActiveAgent {
  terminalId: string
  command: string
  tabId: string
  tabTitle: string
  groupIndex: number
  status: 'active' | 'idle'
  /** Hook-based pane status (more accurate than polling) */
  hookStatus: PaneStatus
}

/** Track in-flight triage calls per project to prevent duplicate launches */
const _triageInFlight = new Set<string>()

/** Last time the Tauri `agent:lifecycle` hook fired for a pane. Used by the
 *  poll-based cleanup to avoid clobbering hook-driven 'working' states while
 *  hooks are actively reporting. A long grace covers quiet Claude turns
 *  (pure thinking, no tool calls) where no hook fires until Stop. */
const _hookEventAt = new Map<string, number>()
const HOOK_TRUST_GRACE_MS = 120_000
const OUTPUT_TRUST_GRACE_MS = 3_000

/** Track agent start times for launch failure detection (paneId → timestamp) */
const _agentStartTimes = new Map<string, number>()
/** Track failed launches to avoid infinite retry loops (paneId → retry count) */
const _launchRetries = new Map<string, number>()
/** Track pending retry timeouts so they can be cancelled on cleanup */
const _retryTimeouts = new Set<ReturnType<typeof setTimeout>>()
const LAUNCH_FAILURE_THRESHOLD_MS = 5000
const MAX_LAUNCH_RETRIES = 1

/** A terminal that needs to be briefly mounted off-screen to spawn its PTY */
export interface BackgroundSpawn {
  id: string
  terminalId: string
  cwd: string
  command: string
  args: string[]
}

interface ActiveAgentsState {
  agents: Map<string, ActiveAgent>
  outputTimestamps: Map<string, number>
  /** Hook-based pane statuses keyed by paneId (terminalId) */
  paneStatuses: Map<string, PaneStatus>
  /** Maps paneId → projectId so we know which project an agent belongs to */
  paneProjectMap: Map<string, string>
  /** Terminals waiting to be briefly mounted off-screen to spawn their PTY */
  backgroundSpawns: BackgroundSpawn[]

  hasActiveAgents: () => boolean
  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  isTerminalRunningAgent: (terminalId: string) => boolean
  getPaneStatus: (paneId: string) => PaneStatus
  getAggregateStatus: () => PaneStatus
  getProjectStatus: (projectId: string) => PaneStatus
  recordOutput: (terminalId: string) => void
  recordTitleActivity: (paneId: string, isWorking: boolean) => void
  handleLifecycleEvent: (paneId: string, tabId: string, eventType: string) => void
  addBackgroundSpawn: (spawn: BackgroundSpawn) => void
  removeBackgroundSpawn: (id: string) => void

  pollOnce: () => Promise<void>
}

export const useActiveAgentsStore = create<ActiveAgentsState>((set, get) => ({
  agents: new Map(),
  outputTimestamps: new Map(),
  paneStatuses: new Map(),
  paneProjectMap: new Map(),
  backgroundSpawns: [],

  addBackgroundSpawn: (spawn: BackgroundSpawn) => {
    set((s) => ({ backgroundSpawns: [...s.backgroundSpawns, spawn] }))
    setTimeout(() => get().removeBackgroundSpawn(spawn.id), 10000)
  },
  removeBackgroundSpawn: (id: string) => {
    set((s) => ({ backgroundSpawns: s.backgroundSpawns.filter((b) => b.id !== id) }))
  },

  hasActiveAgents: () => {
    // Check both polling-detected agents and hook-detected working panes
    const { agents, paneStatuses } = get()
    if (agents.size > 0) return true
    for (const status of paneStatuses.values()) {
      if (status === 'working' || status === 'permission') return true
    }
    return false
  },

  getActiveAgentsList: () => Array.from(get().agents.values()),

  getAgentsInTab: (tabId: string) =>
    Array.from(get().agents.values()).filter((a) => a.tabId === tabId),

  isTerminalRunningAgent: (terminalId: string) => {
    if (get().agents.has(terminalId)) return true
    const hookStatus = get().paneStatuses.get(terminalId)
    return hookStatus === 'working' || hookStatus === 'permission'
  },

  getPaneStatus: (paneId: string) => get().paneStatuses.get(paneId) ?? 'idle',

  /** Get the highest-priority agent status for a specific project. */
  getProjectStatus: (projectId: string): PaneStatus => {
    const { paneStatuses, paneProjectMap } = get()
    let hasWorking = false
    let hasPermission = false
    let hasReview = false
    for (const [paneId, status] of paneStatuses) {
      if (paneProjectMap.get(paneId) === projectId) {
        if (status === 'permission') hasPermission = true
        else if (status === 'working') hasWorking = true
        else if (status === 'review') hasReview = true
      }
    }
    if (hasPermission) return 'permission'
    if (hasWorking) return 'working'
    if (hasReview) return 'review'
    return 'idle'
  },

  /** Get the highest-priority agent status across all panes. */
  getAggregateStatus: (): PaneStatus => {
    const { paneStatuses } = get()
    let hasWorking = false
    let hasPermission = false
    let hasReview = false
    for (const status of paneStatuses.values()) {
      if (status === 'permission') hasPermission = true
      else if (status === 'working') hasWorking = true
      else if (status === 'review') hasReview = true
    }
    // Priority: permission > working > review > idle
    if (hasPermission) return 'permission'
    if (hasWorking) return 'working'
    if (hasReview) return 'review'
    return 'idle'
  },

  recordOutput: (terminalId: string) => {
    get().outputTimestamps.set(terminalId, Date.now())
  },

  /**
   * Light-touch state update from terminal title signals (braille-spinner
   * prefix = working, ✳-family prefix = idle). Only flips between
   * idle ↔ working so it never clobbers 'permission' or 'review' — those
   * come from the Tauri lifecycle hook and have higher priority.
   */
  recordTitleActivity: (paneId: string, isWorking: boolean) => {
    const { paneStatuses, paneProjectMap } = get()
    const current = paneStatuses.get(paneId) ?? 'idle'
    if (current === 'permission' || current === 'review') return
    const next: PaneStatus = isWorking ? 'working' : 'idle'
    if (current === next) return
    const newStatuses = new Map(paneStatuses)
    newStatuses.set(paneId, next)
    set({ paneStatuses: newStatuses })

    // Bind paneId → activeProjectId on the first 'working' transition
    // so getProjectStatus() can attribute the spinner to a workspace
    // (drives the sidebar Active section + IconRail dots). Mirrors
    // what handleLifecycleEvent does on a 'start' hook event — but
    // for v2 panes whose working state comes from terminal-title
    // OSC events rather than from agent lifecycle hooks, this is the
    // only path that populates the map. Without it, paneStatuses
    // says 'working' but no project owns the spinner.
    if (isWorking && !paneProjectMap.has(paneId)) {
      const ps = useProjectsStore.getState()
      if (ps.activeProjectId) {
        const newPaneProjectMap = new Map(paneProjectMap)
        newPaneProjectMap.set(paneId, ps.activeProjectId)
        set({ paneProjectMap: newPaneProjectMap })
        // Also touches lastInteractionAt → 24h Active Bar tenure.
        ps.touchInteraction(ps.activeProjectId)
      }
    }
  },

  handleLifecycleEvent: (paneId: string, _tabId: string, eventType: string) => {
    const toast = useToastStore.getState()
    const { paneStatuses } = get()
    const newStatuses = new Map(paneStatuses)

    // Record the hook fire so the poll-based cleanup doesn't race us.
    _hookEventAt.set(paneId, Date.now())

    if (eventType === 'start') {
      newStatuses.set(paneId, 'working')
      _agentStartTimes.set(paneId, Date.now())
      // Record which project this pane belongs to
      const ps = useProjectsStore.getState()
      if (ps.activeProjectId) {
        const newPaneProjectMap = new Map(get().paneProjectMap)
        newPaneProjectMap.set(paneId, ps.activeProjectId)
        set({ paneProjectMap: newPaneProjectMap })
        // Touch interaction on the active project — this triggers Active Bar
        ps.touchInteraction(ps.activeProjectId)
      }
    } else if (eventType === 'permission') {
      // Skip duplicate permission toast if already in permission state
      const currentStatus = paneStatuses.get(paneId)
      newStatuses.set(paneId, 'permission')
      if (currentStatus === 'permission') {
        set({ paneStatuses: newStatuses })
        return
      }
      // Notify user that agent needs attention
      toast.addToast(
        'An agent needs your permission',
        'info',
        5000,
        {
          label: 'View',
          onClick: () => {
            // Find which tab contains this pane and switch to it
            const tabsState = useTabsStore.getState()
            for (const tab of tabsState.tabs) {
              if (tab.paneGroups.has(paneId)) {
                tabsState.setActiveTab(tab.id)
                break
              }
            }
          },
        }
      )
    } else if (eventType === 'stop') {
      // Skip if already in stop/review/idle state (avoid duplicate toast from multiple stop events)
      const currentStatus = paneStatuses.get(paneId)
      if (currentStatus === 'review' || currentStatus === 'idle') {
        set({ paneStatuses: newStatuses })
        return
      }

      // Check if the pane's tab is currently active
      const tabsState = useTabsStore.getState()
      let isInActiveTab = false
      for (const tab of tabsState.tabs) {
        if (tab.id === tabsState.activeTabId && tab.paneGroups.has(paneId)) {
          isInActiveTab = true
          break
        }
      }
      newStatuses.set(paneId, isInActiveTab ? 'idle' : 'review')

      if (!isInActiveTab) {
        toast.addToast(
          'An agent has finished working',
          'success',
          4000,
          {
            label: 'View',
            onClick: () => {
              for (const tab of tabsState.tabs) {
                if (tab.paneGroups.has(paneId)) {
                  tabsState.setActiveTab(tab.id)
                  // Clear review status when user navigates to it
                  const statuses = new Map(get().paneStatuses)
                  statuses.set(paneId, 'idle')
                  set({ paneStatuses: statuses })
                  break
                }
              }
            },
          }
        )
      }
    }

    set({ paneStatuses: newStatuses })

    // Launch failure detection: if agent stopped within 5s of starting, retry once
    if (eventType === 'stop') {
      const startTime = _agentStartTimes.get(paneId)
      _agentStartTimes.delete(paneId)
      if (startTime && (Date.now() - startTime) < LAUNCH_FAILURE_THRESHOLD_MS) {
        const retries = _launchRetries.get(paneId) || 0
        if (retries < MAX_LAUNCH_RETRIES) {
          _launchRetries.set(paneId, retries + 1)
          const projectId = get().paneProjectMap.get(paneId)
          if (projectId) {
            const project = useProjectsStore.getState().projects.find(p => p.id === projectId)
            if (project) {
              console.warn(`[agent-launch] Agent in ${project.name} failed within ${LAUNCH_FAILURE_THRESHOLD_MS}ms — retrying in 30s (attempt ${retries + 1})`)
              toast.addToast('Agent launch failed — retrying in 30s', 'warning', 5000)
              const retryTimer = setTimeout(() => {
                _retryTimeouts.delete(retryTimer)
                invoke('k2so_agents_triage_decide', { projectPath: project.path }).catch(() => {})
              }, 30000)
              _retryTimeouts.add(retryTimer)
            }
          }
          return // Don't proceed to normal retriage
        } else {
          _launchRetries.delete(paneId)
          console.error(`[agent-launch] Agent launch failed after ${MAX_LAUNCH_RETRIES} retries`)
          toast.addToast('Agent launch failed — check agent configuration', 'error', 8000)
        }
      } else {
        _launchRetries.delete(paneId)
      }
    }

    // Re-triage: if an agent session just stopped, check if there's more work
    // to do for heartbeat-enabled projects (with concurrency guard)
    // Only runs when agentic systems are enabled
    if (eventType === 'stop') {
      const projectId = get().paneProjectMap.get(paneId)
      if (projectId && useSettingsStore.getState().agenticSystemsEnabled) {
        const project = useProjectsStore.getState().projects.find(p => p.id === projectId)
        if (project && project.heartbeatEnabled) {
          // Skip if triage already in flight for this project
          if (_triageInFlight.has(project.path)) return

          // Small delay to let the session clean up, then triage
          setTimeout(() => {
            if (_triageInFlight.has(project.path)) return
            _triageInFlight.add(project.path)

            invoke('k2so_agents_triage_decide', { projectPath: project.path })
              .then((agents: unknown) => {
                const agentList = agents as string[]
                for (const agentName of agentList) {
                  invoke('k2so_agents_build_launch', { projectPath: project.path, agentName })
                    .then((launchInfo: unknown) => {
                      const info = launchInfo as { command: string; args: string[]; cwd: string; agentName: string }
                      useTabsStore.getState().addTab(info.cwd, {
                        title: `Agent: ${info.agentName}`,
                        command: info.command,
                        args: info.args,
                      })
                    })
                    .catch(console.error)
                }
              })
              .catch(console.error)
              .finally(() => { _triageInFlight.delete(project.path) })
          }, 3000)
        }
      }
    }
  },

  pollOnce: async () => {
    const tabsState = useTabsStore.getState()

    // Collect all terminals across all tab groups
    const terminals: Array<{
      terminalId: string
      tabId: string
      tabTitle: string
      groupIndex: number
    }> = []

    // Group 0
    for (const tab of tabsState.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            terminals.push({
              terminalId: data.terminalId,
              tabId: tab.id,
              tabTitle: tab.title,
              groupIndex: 0,
            })
          }
        }
      }
    }

    // Extra groups
    for (let gi = 0; gi < tabsState.extraGroups.length; gi++) {
      const group = tabsState.extraGroups[gi]
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const data = item.data as TerminalItemData
              terminals.push({
                terminalId: data.terminalId,
                tabId: tab.id,
                tabTitle: tab.title,
                groupIndex: gi + 1,
              })
            }
          }
        }
      }
    }

    // Poll each terminal for its foreground command
    const newAgents = new Map<string, ActiveAgent>()

    const now = Date.now()
    const { agents: oldAgents, outputTimestamps } = get()

    await Promise.all(
      terminals.map(async (t) => {
        try {
          const command = await invoke<string | null>('terminal_get_foreground_command', { id: t.terminalId })
          if (command && KNOWN_AGENT_COMMANDS.has(command)) {
            const lastOutput = outputTimestamps.get(t.terminalId) ?? 0
            const status: 'active' | 'idle' = (now - lastOutput < AGENT_IDLE_THRESHOLD_MS) ? 'active' : 'idle'

            newAgents.set(t.terminalId, {
              terminalId: t.terminalId,
              command,
              tabId: t.tabId,
              tabTitle: t.tabTitle,
              groupIndex: t.groupIndex,
              status,
              hookStatus: get().paneStatuses.get(t.terminalId) ?? 'idle',
            })
          }
        } catch {
          // Terminal may have been killed — ignore
        }
      })
    )

    // Detect transitions and fire toasts
    const toast = useToastStore.getState()

    // Only fire poll-based toasts if the hook system is NOT active for this pane.
    // When hooks are working, handleLifecycleEvent handles all toasts.
    const { paneStatuses } = get()

    for (const [terminalId, newAgent] of newAgents) {
      const oldAgent = oldAgents.get(terminalId)
      if (oldAgent?.status === 'active' && newAgent.status === 'idle') {
        if (!paneStatuses.has(terminalId)) {
          const { tabId, groupIndex } = newAgent
          toast.addToast(
            `${newAgent.command} is waiting for input in "${newAgent.tabTitle}"`,
            'info',
            5000,
            {
              label: 'Switch to tab',
              onClick: () => useTabsStore.getState().setActiveTabInGroup(groupIndex, tabId),
            }
          )
        }
      }
    }

    for (const [terminalId, oldAgent] of oldAgents) {
      if (!newAgents.has(terminalId) && oldAgent.status === 'active') {
        if (!paneStatuses.has(terminalId)) {
          const { tabId, groupIndex } = oldAgent
          toast.addToast(
            `${oldAgent.command} finished in "${oldAgent.tabTitle}"`,
            'success',
            4000,
            {
              label: 'Switch to tab',
              onClick: () => useTabsStore.getState().setActiveTabInGroup(groupIndex, tabId),
            }
          )
        }
      }
    }

    // Clean up output timestamps and pane statuses for terminals no longer running an agent.
    // This handles the case where the user interrupts (Esc) and the hook 'stop' event never fires.
    const cleanedStatuses = new Map(paneStatuses)
    let statusesChanged = false
    for (const terminalId of outputTimestamps.keys()) {
      if (newAgents.has(terminalId)) continue
      // Preserve the timestamp for panes with an active hook-driven
      // status — it's the OUTPUT_TRUST_GRACE_MS signal that keeps the
      // working state alive through the cleanup branch below. v2
      // (Alacritty) panes never appear in `newAgents` because
      // `terminal_get_foreground_command` only sees legacy
      // TerminalManager sessions; without this exemption their
      // 'working' state would get clobbered on every poll cycle and
      // the braille spinner would never stay lit. Hook-driven legacy
      // panes are unaffected (they're already in newAgents via the
      // KNOWN_AGENT_COMMANDS check).
      if (paneStatuses.has(terminalId)) continue
      outputTimestamps.delete(terminalId)
    }
    const cleanupNow = Date.now()
    for (const [paneId, status] of paneStatuses) {
      if ((status === 'working' || status === 'permission') && !newAgents.has(paneId)) {
        // The foreground command isn't a known agent — but that fires
        // transiently during Claude's tool-use (child process briefly
        // runs `bash`, `rg`, etc.). Only clear when *both* trust signals
        // have gone quiet: hooks haven't fired in a long time AND output
        // isn't flowing. Otherwise we clobber a legitimate working state.
        const hookAge = cleanupNow - (_hookEventAt.get(paneId) ?? 0)
        const outputAge = cleanupNow - (outputTimestamps.get(paneId) ?? 0)
        if (hookAge < HOOK_TRUST_GRACE_MS) continue
        if (outputAge < OUTPUT_TRUST_GRACE_MS) continue
        cleanedStatuses.set(paneId, 'idle')
        _hookEventAt.delete(paneId)
        statusesChanged = true
      }
    }

    if (statusesChanged) {
      set({ agents: newAgents, paneStatuses: cleanedStatuses })
    } else {
      set({ agents: newAgents })
    }
  },
}))

// ── Polling ─────────────────────────────────────────────────────────
let pollInterval: ReturnType<typeof setInterval> | null = null
let hookUnlisten: (() => void) | null = null

export function startAgentPolling(): void {
  if (pollInterval) return
  // Initial poll
  useActiveAgentsStore.getState().pollOnce()
  // Add jitter to avoid thundering-herd across multiple windows
  const interval = 2500 + Math.floor(Math.random() * 500)
  pollInterval = setInterval(() => {
    useActiveAgentsStore.getState().pollOnce()
  }, interval)

  // Helper: create a tab for a companion-spawned terminal that's already running.
  // Adds the tab to the current workspace without switching active tab.
  function createCompanionTab(terminalId: string, command: string, cwd: string) {
    const tabsStore = useTabsStore.getState()

    // Check if a tab for this terminal already exists
    const exists = tabsStore.tabs.some((t) =>
      [...t.paneGroups.values()].some((pg) =>
        pg.items.some((item) => item.type === 'terminal' && (item.data as any).terminalId === terminalId)
      )
    )
    if (exists) return

    // Save current active tab so we can restore it after addTab switches
    const currentActiveTabId = tabsStore.activeTabId
    const cmd = command.split(' ')[0] || 'shell'
    tabsStore.addTab(cwd, {
      title: `Companion: ${cmd}`,
      command: cmd,
      args: command.split(' ').slice(1),
    })

    // Override the new tab's terminal ID to connect to the existing PTY
    const updatedStore = useTabsStore.getState()
    const newTab = updatedStore.tabs[updatedStore.tabs.length - 1]
    if (newTab) {
      const pg = [...newTab.paneGroups.values()][0]
      if (pg?.items[0]?.data) {
        (pg.items[0].data as any).terminalId = terminalId
      }
      const oldPgId = pg?.id
      if (oldPgId && oldPgId !== terminalId) {
        newTab.paneGroups.delete(oldPgId)
        pg.id = terminalId
        newTab.paneGroups.set(terminalId, pg)
        newTab.mosaicTree = terminalId
      }
    }

    // Restore the previously active tab — don't switch to the new one
    if (currentActiveTabId) {
      useTabsStore.setState({ activeTabId: currentActiveTabId })
    }
  }

  // Listen for hook-based lifecycle events from the Rust notification server
  import('@tauri-apps/api/event').then(({ listen }) => {
    listen<{ paneId: string; tabId: string; eventType: string }>('agent:lifecycle', (event) => {
      const { paneId, tabId, eventType } = event.payload
      useActiveAgentsStore.getState().handleLifecycleEvent(paneId, tabId, eventType)
    }).then((fn) => {
      hookUnlisten = fn
    })

    // Surface hook-injection failures — previously these were debug-only
    // log lines, so users never learned that e.g. a malformed
    // ~/.claude/settings.json had silently broken their spinner pipeline.
    // One toast per startup, listing which CLIs failed.
    listen<{ failures: Array<{ cli: string; error: string }> }>('hook-injection-failed', (event) => {
      const failures = event.payload?.failures ?? []
      if (failures.length === 0) return
      const clis = failures.map((f) => f.cli).join(', ')
      useToastStore.getState().addToast(
        `Hook injection failed for ${clis} — run \`k2so hooks status\` for details`,
        'warning',
        10000,
      )
    })

    // Listen for CLI-triggered agent launch requests
    listen<{ command: string; args: string[]; cwd: string; agentName: string; worktreePath?: string }>('cli:agent-launch', async (event) => {
      const { command, args, cwd, agentName, worktreePath } = event.payload
      const tabOpts = { title: `Agent: ${agentName}`, command, args }

      // If this launch is for a worktree, create the PTY in the background
      // with the same terminal ID the Chat tab will use (agent-chat-wt-{wsId}).
      if (worktreePath) {
        // Wait briefly for sync:projects to register the new workspace
        let wsId: string | null = null
        for (let attempt = 0; attempt < 10; attempt++) {
          const projectsStore = useProjectsStore.getState()
          for (const project of projectsStore.projects) {
            const ws = project.workspaces.find((w) => w.worktreePath === worktreePath)
            if (ws) { wsId = ws.id; break }
          }
          if (wsId) break
          await new Promise((r) => setTimeout(r, 500))
        }

        if (wsId) {
          const bgTerminalId = `agent-chat-wt-${wsId}`
          try {
            const exists = await invoke<boolean>('terminal_exists', { id: bgTerminalId })
            if (!exists) {
              await invoke('terminal_create', { cwd, command, args, id: bgTerminalId })
            }
            // Register system-managed worktree session
            invoke('k2so_agents_lock', {
              projectPath: cwd,
              agentName,
              terminalId: bgTerminalId,
              owner: 'system',
            }).catch(() => {})
          } catch { /* will be created when user navigates */ }
        }
        return
      }

      // For agent launches without a worktree (e.g. manager), create the PTY
      // in the background with a deterministic ID. The Chat tab discovers it via
      // terminal_list_running_agents when the user navigates there.
      const bgTerminalId = `agent-chat-${agentName}`
      try {
        const exists = await invoke<boolean>('terminal_exists', { id: bgTerminalId })
        if (!exists) {
          await invoke('terminal_create', {
            cwd,
            command,
            args,
            id: bgTerminalId,
          })
        }
        // Register system-managed session in DB (owner='system' so scheduler knows)
        invoke('k2so_agents_lock', {
          projectPath: cwd,
          agentName,
          terminalId: bgTerminalId,
          owner: 'system',
        }).catch(() => {})
        // Detect and save session ID after a brief delay
        setTimeout(async () => {
          try {
            const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
              provider: 'claude',
              projectPath: cwd,
            })
            if (sessionId) {
              invoke('k2so_agents_save_session_id', {
                projectPath: cwd,
                agentName,
                sessionId,
              }).catch(() => {})
            }
          } catch { /* ignore */ }
        }, 5000)
      } catch {
        // Fallback: add tab to current workspace if background creation fails
        const tabsStore = useTabsStore.getState()
        tabsStore.addTabToGroup(tabsStore.activeGroupIndex, cwd, tabOpts)
      }
    })

    // Listen for CLI-triggered sub-terminal spawn requests (multi-terminal execution)
    listen<{ agentName: string; command: string; cwd: string; title: string; wait: boolean; projectPath: string }>(
      'cli:terminal-spawn', (event) => {
        const { agentName, command, cwd, title } = event.payload
        const tabsStore = useTabsStore.getState()

        // Find the agent's existing tab (look for "Agent: <name>" title)
        const agentTab = tabsStore.tabs.find((t) =>
          t.title === `Agent: ${agentName}` || t.paneGroups.values().next().value?.panes?.some(
            (p: any) => p.title === `Agent: ${agentName}`
          )
        )

        if (agentTab) {
          // Split within the existing agent tab
          const activeGroup = tabsStore.activeGroupIndex
          tabsStore.addTabToGroup(activeGroup, cwd, {
            title: `${agentName}: ${title}`,
            command: command.split(' ')[0],
            args: command.split(' ').slice(1),
          })
        } else {
          // No agent tab found — create new tab
          const activeGroup = tabsStore.activeGroupIndex
          tabsStore.addTabToGroup(activeGroup, cwd, {
            title: `${agentName}: ${title}`,
            command: command.split(' ')[0],
            args: command.split(' ').slice(1),
          })
        }
      }
    )

    // Companion background terminal spawn — queue for tab creation.
    // If the target workspace is active, create the tab immediately.
    // If not, queue it and create when the user switches to that workspace.
    const pendingCompanionTerminals: Array<{
      terminalId: string
      command: string
      cwd: string
      projectPath: string
    }> = []

    listen<{ terminalId: string; command: string; cwd: string; projectPath: string }>(
      'cli:terminal-spawn-background', (event) => {
        const { terminalId, command, cwd, projectPath } = event.payload

        // Find which project this terminal belongs to
        const projects = useProjectsStore.getState().projects
        const project = projects.find((p) => cwd.startsWith(p.path))
        const activeProjectId = useProjectsStore.getState().activeProjectId

        if (project && project.id === activeProjectId) {
          // Target workspace is active — create tab immediately (without switching to it)
          createCompanionTab(terminalId, command, cwd)
        } else {
          // Queue for later — will be created when user switches to this workspace
          pendingCompanionTerminals.push({ terminalId, command, cwd, projectPath })
        }
      }
    )

    // Watch for workspace switches — flush any pending companion terminals
    useProjectsStore.subscribe((state, prevState) => {
      if (state.activeProjectId && state.activeProjectId !== prevState.activeProjectId) {
        const project = state.projects.find((p) => p.id === state.activeProjectId)
        if (!project) return

        // Find and create tabs for any pending terminals in this workspace
        const toCreate = pendingCompanionTerminals.filter((t) => t.cwd.startsWith(project.path))
        for (const t of toCreate) {
          // Brief delay to let restoreWorkspace finish
          setTimeout(() => createCompanionTab(t.terminalId, t.command, t.cwd), 300)
        }
        // Remove created terminals from pending list
        for (let i = pendingCompanionTerminals.length - 1; i >= 0; i--) {
          if (pendingCompanionTerminals[i].cwd.startsWith(project.path)) {
            pendingCompanionTerminals.splice(i, 1)
          }
        }
      }
    })

    // Listen for CLI-triggered AI Commit requests
    listen<{
      projectPath: string
      includeMerge: boolean
      message: string
      gitContext: {
        branch: string
        status: string
        diffStat: string
        stagedStat: string
        recentLog: string
      }
    }>('cli:ai-commit', (event) => {
      const { projectPath, includeMerge, message, gitContext } = event.payload
      const defaultAgent = useSettingsStore.getState().defaultAgent
      const presets = usePresetsStore.getState().presets
      const preset = presets.find((p) => p.id === defaultAgent)
      if (!preset) return

      const { command, args } = parseCommand(preset.command)

      // Build a rich prompt with git context so the agent has immediate visibility
      const parts: string[] = []

      if (message) {
        parts.push(message)
      } else {
        parts.push('Create a commit for the changes in this repository.')
      }

      parts.push('')
      parts.push('## Current State')
      parts.push(`Branch: ${gitContext.branch || 'unknown'}`)

      if (gitContext.stagedStat) {
        parts.push('')
        parts.push('### Staged Changes')
        parts.push('```')
        parts.push(gitContext.stagedStat)
        parts.push('```')
      }

      if (gitContext.diffStat) {
        parts.push('')
        parts.push('### Unstaged Changes')
        parts.push('```')
        parts.push(gitContext.diffStat)
        parts.push('```')
      }

      if (gitContext.status) {
        parts.push('')
        parts.push('### git status')
        parts.push('```')
        parts.push(gitContext.status)
        parts.push('```')
      }

      if (gitContext.recentLog) {
        parts.push('')
        parts.push('### Recent Commits (for style reference)')
        parts.push('```')
        parts.push(gitContext.recentLog)
        parts.push('```')
      }

      parts.push('')
      parts.push('## Instructions')
      parts.push('1. Review the diff carefully with `git diff` and `git diff --cached`')
      parts.push('2. Stage any unstaged files that should be included (use `git add <file>`, not `git add -A`)')
      parts.push('3. Write a clear, concise commit message that explains the **why**, matching the style of recent commits above')
      parts.push('4. Commit the changes')

      if (includeMerge) {
        parts.push('5. After committing, merge this branch into main and resolve any conflicts')
      }

      const prompt = parts.join('\n')

      const tabsStore = useTabsStore.getState()
      const activeGroup = tabsStore.activeGroupIndex
      tabsStore.addTabToGroup(activeGroup, projectPath, {
        title: includeMerge ? 'AI Commit & Merge' : 'AI Commit',
        command,
        args: [...args, prompt]
      })
    })
  })
}

export function stopAgentPolling(): void {
  if (pollInterval) {
    clearInterval(pollInterval)
    pollInterval = null
  }
  if (hookUnlisten) {
    hookUnlisten()
    hookUnlisten = null
  }
  // Cancel any pending retry timeouts to prevent ghost launches
  for (const timer of _retryTimeouts) {
    clearTimeout(timer)
  }
  _retryTimeouts.clear()
  _agentStartTimes.clear()
  _launchRetries.clear()
}

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

interface ActiveAgentsState {
  agents: Map<string, ActiveAgent>
  outputTimestamps: Map<string, number>
  /** Hook-based pane statuses keyed by paneId (terminalId) */
  paneStatuses: Map<string, PaneStatus>
  /** Maps paneId → projectId so we know which project an agent belongs to */
  paneProjectMap: Map<string, string>

  hasActiveAgents: () => boolean
  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  isTerminalRunningAgent: (terminalId: string) => boolean
  getPaneStatus: (paneId: string) => PaneStatus
  getAggregateStatus: () => PaneStatus
  getProjectStatus: (projectId: string) => PaneStatus
  recordOutput: (terminalId: string) => void
  handleLifecycleEvent: (paneId: string, tabId: string, eventType: string) => void

  pollOnce: () => Promise<void>
}

export const useActiveAgentsStore = create<ActiveAgentsState>((set, get) => ({
  agents: new Map(),
  outputTimestamps: new Map(),
  paneStatuses: new Map(),
  paneProjectMap: new Map(),

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

  handleLifecycleEvent: (paneId: string, _tabId: string, eventType: string) => {
    const toast = useToastStore.getState()
    const { paneStatuses } = get()
    const newStatuses = new Map(paneStatuses)

    if (eventType === 'start') {
      newStatuses.set(paneId, 'working')
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
      newStatuses.set(paneId, 'permission')
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

    // Re-triage: if an agent session just stopped, check if there's more work
    // to do for heartbeat-enabled projects
    if (eventType === 'stop') {
      const projectId = get().paneProjectMap.get(paneId)
      if (projectId) {
        const project = useProjectsStore.getState().projects.find(p => p.id === projectId)
        if (project && project.heartbeatEnabled) {
          // Small delay to let the session clean up, then triage
          setTimeout(() => {
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

    // Clean up output timestamps for terminals no longer tracked
    for (const terminalId of outputTimestamps.keys()) {
      if (!newAgents.has(terminalId)) {
        outputTimestamps.delete(terminalId)
      }
    }

    set({ agents: newAgents })
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

  // Listen for hook-based lifecycle events from the Rust notification server
  import('@tauri-apps/api/event').then(({ listen }) => {
    listen<{ paneId: string; tabId: string; eventType: string }>('agent:lifecycle', (event) => {
      const { paneId, tabId, eventType } = event.payload
      useActiveAgentsStore.getState().handleLifecycleEvent(paneId, tabId, eventType)
    }).then((fn) => {
      hookUnlisten = fn
    })

    // Listen for CLI-triggered agent launch requests
    listen<{ command: string; args: string[]; cwd: string; agentName: string }>('cli:agent-launch', (event) => {
      const { command, args, cwd, agentName } = event.payload
      const tabsStore = useTabsStore.getState()
      const activeGroup = tabsStore.activeGroupIndex
      tabsStore.addTabToGroup(activeGroup, cwd, {
        title: `Agent: ${agentName}`,
        command,
        args
      })
    })

    // Listen for CLI-triggered AI Commit requests
    listen<{ projectPath: string; includeMerge: boolean; message: string }>('cli:ai-commit', (event) => {
      const { projectPath, includeMerge, message } = event.payload
      const defaultAgent = useSettingsStore.getState().defaultAgent
      const presets = usePresetsStore.getState().presets
      const preset = presets.find((p) => p.id === defaultAgent)
      if (!preset) return

      const { command, args } = parseCommand(preset.command)

      // Build prompt for the AI agent
      let prompt = message || 'Review the changes in this repository and create a well-structured commit with an appropriate commit message.'
      if (includeMerge) {
        prompt += '\n\nAfter committing, merge this branch back into main and resolve any conflicts.'
      }

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
}

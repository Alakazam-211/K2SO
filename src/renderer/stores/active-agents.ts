import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore, type TerminalItemData } from './tabs'
import { useToastStore } from './toast'
import { KNOWN_AGENT_COMMANDS, AGENT_IDLE_THRESHOLD_MS } from '@shared/constants'

export interface ActiveAgent {
  terminalId: string
  command: string
  tabId: string
  tabTitle: string
  groupIndex: number
  status: 'active' | 'idle'
}

interface ActiveAgentsState {
  agents: Map<string, ActiveAgent>
  outputTimestamps: Map<string, number>

  hasActiveAgents: () => boolean
  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  isTerminalRunningAgent: (terminalId: string) => boolean
  recordOutput: (terminalId: string) => void

  pollOnce: () => Promise<void>
}

export const useActiveAgentsStore = create<ActiveAgentsState>((set, get) => ({
  agents: new Map(),
  outputTimestamps: new Map(),

  hasActiveAgents: () => get().agents.size > 0,

  getActiveAgentsList: () => Array.from(get().agents.values()),

  getAgentsInTab: (tabId: string) =>
    Array.from(get().agents.values()).filter((a) => a.tabId === tabId),

  isTerminalRunningAgent: (terminalId: string) => get().agents.has(terminalId),

  recordOutput: (terminalId: string) => {
    get().outputTimestamps.set(terminalId, Date.now())
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
            })
          }
        } catch {
          // Terminal may have been killed — ignore
        }
      })
    )

    // Detect transitions and fire toasts
    const toast = useToastStore.getState()

    for (const [terminalId, newAgent] of newAgents) {
      const oldAgent = oldAgents.get(terminalId)
      if (oldAgent?.status === 'active' && newAgent.status === 'idle') {
        // Agent was actively working, now idle → waiting for input
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

    for (const [terminalId, oldAgent] of oldAgents) {
      if (!newAgents.has(terminalId) && oldAgent.status === 'active') {
        // Agent was actively working and has now exited
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

export function startAgentPolling(): void {
  if (pollInterval) return
  // Initial poll
  useActiveAgentsStore.getState().pollOnce()
  pollInterval = setInterval(() => {
    useActiveAgentsStore.getState().pollOnce()
  }, 2500)
}

export function stopAgentPolling(): void {
  if (pollInterval) {
    clearInterval(pollInterval)
    pollInterval = null
  }
}

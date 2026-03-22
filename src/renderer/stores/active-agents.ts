import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore, type TerminalItemData } from './tabs'
import { KNOWN_AGENT_COMMANDS } from '@shared/constants'

export interface ActiveAgent {
  terminalId: string
  command: string
  tabId: string
  tabTitle: string
  groupIndex: number
}

interface ActiveAgentsState {
  agents: Map<string, ActiveAgent>

  hasActiveAgents: () => boolean
  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  isTerminalRunningAgent: (terminalId: string) => boolean

  pollOnce: () => Promise<void>
}

export const useActiveAgentsStore = create<ActiveAgentsState>((set, get) => ({
  agents: new Map(),

  hasActiveAgents: () => get().agents.size > 0,

  getActiveAgentsList: () => Array.from(get().agents.values()),

  getAgentsInTab: (tabId: string) =>
    Array.from(get().agents.values()).filter((a) => a.tabId === tabId),

  isTerminalRunningAgent: (terminalId: string) => get().agents.has(terminalId),

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

    await Promise.all(
      terminals.map(async (t) => {
        try {
          const command = await invoke<string | null>('terminal_get_foreground_command', { id: t.terminalId })
          if (command && KNOWN_AGENT_COMMANDS.has(command)) {
            newAgents.set(t.terminalId, {
              terminalId: t.terminalId,
              command,
              tabId: t.tabId,
              tabTitle: t.tabTitle,
              groupIndex: t.groupIndex,
            })
          }
        } catch {
          // Terminal may have been killed — ignore
        }
      })
    )

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

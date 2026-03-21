import { create } from 'zustand'
import { trpc } from '@/lib/trpc'
import { useTabsStore } from './tabs'
import type { TerminalPane } from './tabs'

// ── Types ────────────────────────────────────────────────────────────────

export interface AgentPreset {
  id: string
  label: string
  command: string
  icon: string | null
  enabled: number
  sortOrder: number
  isBuiltIn: number
  createdAt: number
}

interface PresetsState {
  presets: AgentPreset[]
  showPresetsBar: boolean
  fetchPresets: () => Promise<void>
  togglePresetsBar: () => void
  launchPreset: (presetId: string, cwd: string, mode: 'tab' | 'split') => void
}

// ── Helpers ──────────────────────────────────────────────────────────────

function parseCommand(commandStr: string): { command: string; args: string[] } {
  // Split respecting quoted strings
  const parts: string[] = []
  let current = ''
  let inQuote: string | null = null

  for (let i = 0; i < commandStr.length; i++) {
    const ch = commandStr[i]

    if (inQuote) {
      if (ch === inQuote) {
        inQuote = null
      } else {
        current += ch
      }
    } else if (ch === '"' || ch === "'") {
      inQuote = ch
    } else if (ch === ' ') {
      if (current.length > 0) {
        parts.push(current)
        current = ''
      }
    } else {
      current += ch
    }
  }
  if (current.length > 0) {
    parts.push(current)
  }

  const [command, ...args] = parts
  return { command: command || '', args }
}

// ── Store ────────────────────────────────────────────────────────────────

export const usePresetsStore = create<PresetsState>((set, get) => ({
  presets: [],
  showPresetsBar: true,

  fetchPresets: async () => {
    try {
      const result = await trpc.presets.list.query()
      set({ presets: result })
    } catch (err) {
      console.error('Failed to fetch presets:', err)
    }
  },

  togglePresetsBar: () => {
    set((state) => ({ showPresetsBar: !state.showPresetsBar }))
  },

  launchPreset: (presetId: string, cwd: string, mode: 'tab' | 'split') => {
    const preset = get().presets.find((p) => p.id === presetId)
    if (!preset) return

    const { command, args } = parseCommand(preset.command)
    const tabsStore = useTabsStore.getState()

    if (mode === 'tab') {
      // Create a new tab with the preset command
      const tabId = crypto.randomUUID()
      const paneId = crypto.randomUUID()

      const pane: TerminalPane = {
        type: 'terminal',
        terminalId: paneId,
        cwd,
        command,
        args
      }

      // Use the tabs store internal pattern: manually construct the tab
      const tab = {
        id: tabId,
        title: preset.label,
        mosaicTree: paneId as any,
        panes: new Map([[paneId, pane]])
      }

      // Directly push into tabs store
      useTabsStore.setState((state) => ({
        tabs: [...state.tabs, tab],
        activeTabId: tabId
      }))
    } else {
      // Split mode: split the active tab
      const activeTab = tabsStore.tabs.find((t) => t.id === tabsStore.activeTabId)
      if (!activeTab) {
        // No active tab, create one instead
        get().launchPreset(presetId, cwd, 'tab')
        return
      }

      const firstPaneId = getFirstLeaf(activeTab.mosaicTree)
      if (!firstPaneId) return

      const newPaneId = crypto.randomUUID()
      const newPane: TerminalPane = {
        type: 'terminal',
        terminalId: newPaneId,
        cwd,
        command,
        args
      }

      tabsStore.splitPane(activeTab.id, firstPaneId, newPaneId, newPane, 'column')
    }
  }
}))

// ── Tree helpers ─────────────────────────────────────────────────────────

function getFirstLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getFirstLeaf((tree as { first: unknown }).first)
  }
  return null
}

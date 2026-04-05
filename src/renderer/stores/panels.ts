import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { invoke } from '@tauri-apps/api/core'
import type { AppSettingsResponse } from '@shared/types'

type PanelTab = 'files' | 'changes' | 'history' | 'workspace'

interface PanelsState {
  // Left auxiliary panel (between projects sidebar and terminal)
  leftPanelOpen: boolean
  leftPanelWidth: number
  leftPanelActiveTab: PanelTab
  leftPanelTabs: PanelTab[]

  // Right auxiliary panel (right of terminal)
  rightPanelOpen: boolean
  rightPanelWidth: number
  rightPanelActiveTab: PanelTab
  rightPanelTabs: PanelTab[]

  // Focus window: which side shows the workspace header
  focusWorkspaceHeaderSide: 'left' | 'right'

  // Actions
  toggleLeftPanel: () => void
  toggleRightPanel: () => void
  setLeftPanelWidth: (width: number) => void
  setRightPanelWidth: (width: number) => void
  setLeftPanelActiveTab: (tab: PanelTab) => void
  setRightPanelActiveTab: (tab: PanelTab) => void

  // Move a tab from one side to the other
  moveTabToLeft: (tab: PanelTab) => void
  moveTabToRight: (tab: PanelTab) => void

  // Move workspace header between sides (focus window)
  moveFocusWorkspaceHeader: (side: 'left' | 'right') => void

  /** Activate a specific tab on whichever side has it, opening the panel if needed. */
  activateTab: (tab: PanelTab) => void

  initFromSettings: () => Promise<void>
}

let panelsInitialized = false

export const usePanelsStore = create<PanelsState>((set, get) => ({
  leftPanelOpen: true,
  leftPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  leftPanelActiveTab: 'files',
  leftPanelTabs: ['files', 'workspace'],

  rightPanelOpen: true,
  rightPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  rightPanelActiveTab: 'history',
  rightPanelTabs: ['history', 'changes'],

  focusWorkspaceHeaderSide: 'left',

  toggleLeftPanel: () => {
    const next = !get().leftPanelOpen
    set({ leftPanelOpen: next })
    invoke('settings_update', { updates: { leftPanelOpen: next } }).catch((e: unknown) => console.error('[panels]', e))
  },
  toggleRightPanel: () => {
    const next = !get().rightPanelOpen
    set({ rightPanelOpen: next })
    invoke('settings_update', { updates: { rightPanelOpen: next } }).catch((e: unknown) => console.error('[panels]', e))
  },

  setLeftPanelWidth: (width) =>
    set({ leftPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),
  setRightPanelWidth: (width) =>
    set({ rightPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  setLeftPanelActiveTab: (tab) => {
    set({ leftPanelActiveTab: tab })
    invoke('settings_update', { updates: { leftPanelActiveTab: tab } }).catch((e: unknown) => console.error('[panels]', e))
  },
  setRightPanelActiveTab: (tab) => {
    set({ rightPanelActiveTab: tab })
    invoke('settings_update', { updates: { rightPanelActiveTab: tab } }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveTabToLeft: (tab) => {
    set((s) => ({
      leftPanelTabs: s.leftPanelTabs.includes(tab) ? s.leftPanelTabs : [...s.leftPanelTabs, tab],
      rightPanelTabs: s.rightPanelTabs.filter((t) => t !== tab),
      leftPanelActiveTab: tab,
      rightPanelActiveTab:
        s.rightPanelActiveTab === tab
          ? s.rightPanelTabs.find((t) => t !== tab) ?? s.rightPanelActiveTab
          : s.rightPanelActiveTab
    }))
    const s = get()
    invoke('settings_update', { updates: {
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    } }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveTabToRight: (tab) => {
    set((s) => ({
      rightPanelTabs: s.rightPanelTabs.includes(tab)
        ? s.rightPanelTabs
        : [...s.rightPanelTabs, tab],
      leftPanelTabs: s.leftPanelTabs.filter((t) => t !== tab),
      rightPanelActiveTab: tab,
      leftPanelActiveTab:
        s.leftPanelActiveTab === tab
          ? s.leftPanelTabs.find((t) => t !== tab) ?? s.leftPanelActiveTab
          : s.leftPanelActiveTab
    }))
    const s = get()
    invoke('settings_update', { updates: {
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    } }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveFocusWorkspaceHeader: (side) => {
    set({ focusWorkspaceHeaderSide: side })
  },

  activateTab: (tab: PanelTab) => {
    const state = get()
    // Check left panel first
    if (state.leftPanelTabs.includes(tab)) {
      set({ leftPanelOpen: true, leftPanelActiveTab: tab })
      return
    }
    // Then right panel
    if (state.rightPanelTabs.includes(tab)) {
      set({ rightPanelOpen: true, rightPanelActiveTab: tab })
      return
    }
    // Tab not on either side — add to right panel
    set({
      rightPanelOpen: true,
      rightPanelTabs: [...state.rightPanelTabs, tab],
      rightPanelActiveTab: tab,
    })
  },

  initFromSettings: async () => {
    try {
      const settings = await invoke<AppSettingsResponse>('settings_get')

      // Migrate old tab names to new ones
      const VALID_TABS: PanelTab[] = ['files', 'changes', 'history', 'workspace']
      const migrateTab = (t: string): PanelTab | null => {
        if (t === 'agents' || t === 'reviews') return 'workspace'
        if (VALID_TABS.includes(t as PanelTab)) return t as PanelTab
        return null
      }
      const migrateTabs = (tabs: string[]): PanelTab[] => {
        const mapped = tabs.map(migrateTab).filter((t): t is PanelTab => t !== null)
        // Deduplicate while preserving order
        return [...new Set(mapped)]
      }

      let leftTabs = settings.leftPanelTabs?.length
        ? migrateTabs(settings.leftPanelTabs as string[])
        : ['files', 'workspace'] as PanelTab[]
      let rightTabs = settings.rightPanelTabs?.length
        ? migrateTabs(settings.rightPanelTabs as string[])
        : ['history', 'changes'] as PanelTab[]

      // Ensure 'workspace' only appears on one side — prefer left
      if (leftTabs.includes('workspace') && rightTabs.includes('workspace')) {
        rightTabs = rightTabs.filter((t) => t !== 'workspace')
      }

      let leftActive = settings.leftPanelActiveTab
        ? (migrateTab(settings.leftPanelActiveTab as string) ?? leftTabs[0])
        : leftTabs[0]
      let rightActive = settings.rightPanelActiveTab
        ? (migrateTab(settings.rightPanelActiveTab as string) ?? rightTabs[0])
        : rightTabs[0]

      // Ensure active tabs are valid for their side
      if (!leftTabs.includes(leftActive)) leftActive = leftTabs[0]
      if (!rightTabs.includes(rightActive)) rightActive = rightTabs[0]

      set({
        leftPanelOpen: settings.leftPanelOpen,
        rightPanelOpen: settings.rightPanelOpen,
        leftPanelActiveTab: leftActive,
        rightPanelActiveTab: rightActive,
        leftPanelTabs: leftTabs,
        rightPanelTabs: rightTabs,
      })

      // Only persist on first init to avoid sync:settings → initFromSettings → settings_update loop
      if (!panelsInitialized) {
        panelsInitialized = true
        setTimeout(() => {
          invoke('settings_update', { updates: {
            leftPanelTabs: leftTabs,
            rightPanelTabs: rightTabs,
            leftPanelActiveTab: leftActive,
            rightPanelActiveTab: rightActive,
          } }).catch((e: unknown) => console.error('[panels] migration persist failed:', e))
        }, 2000)
      }
    } catch {
      // ignore — use defaults
    }
  }
}))

// Initialize on import
usePanelsStore.getState().initFromSettings()

import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { invoke } from '@tauri-apps/api/core'

type PanelTab = 'files' | 'changes' | 'history'

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

  initFromSettings: () => Promise<void>
}

export const usePanelsStore = create<PanelsState>((set, get) => ({
  leftPanelOpen: false,
  leftPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  leftPanelActiveTab: 'files',
  leftPanelTabs: ['files'],

  rightPanelOpen: false,
  rightPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  rightPanelActiveTab: 'history',
  rightPanelTabs: ['history', 'changes'],

  toggleLeftPanel: () => {
    const next = !get().leftPanelOpen
    set({ leftPanelOpen: next })
    invoke('settings_update', { leftPanelOpen: next }).catch((e: unknown) => console.error('[panels]', e))
  },
  toggleRightPanel: () => {
    const next = !get().rightPanelOpen
    set({ rightPanelOpen: next })
    invoke('settings_update', { rightPanelOpen: next }).catch((e: unknown) => console.error('[panels]', e))
  },

  setLeftPanelWidth: (width) =>
    set({ leftPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),
  setRightPanelWidth: (width) =>
    set({ rightPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  setLeftPanelActiveTab: (tab) => {
    set({ leftPanelActiveTab: tab })
    invoke('settings_update', { leftPanelActiveTab: tab }).catch((e: unknown) => console.error('[panels]', e))
  },
  setRightPanelActiveTab: (tab) => {
    set({ rightPanelActiveTab: tab })
    invoke('settings_update', { rightPanelActiveTab: tab }).catch((e: unknown) => console.error('[panels]', e))
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
    invoke('settings_update', {
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    }).catch((e: unknown) => console.error('[panels]', e))
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
    invoke('settings_update', {
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    }).catch((e: unknown) => console.error('[panels]', e))
  },

  initFromSettings: async () => {
    try {
      const settings = await invoke<any>('settings_get')
      set({
        leftPanelOpen: settings.leftPanelOpen,
        rightPanelOpen: settings.rightPanelOpen
      })
    } catch {
      // ignore — use defaults
    }
  }
}))

// Initialize on import
usePanelsStore.getState().initFromSettings()

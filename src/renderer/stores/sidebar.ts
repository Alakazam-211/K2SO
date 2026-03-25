import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { invoke } from '@tauri-apps/api/core'

interface SidebarState {
  isCollapsed: boolean
  width: number

  toggle: () => void
  setWidth: (width: number) => void
  collapse: () => void
  expand: () => void
  initFromSettings: () => Promise<void>
}

export const useSidebarStore = create<SidebarState>((set, get) => ({
  isCollapsed: false,
  width: SIDEBAR_DEFAULT_WIDTH,

  toggle: () => {
    const next = !get().isCollapsed
    set({ isCollapsed: next })
    invoke('settings_update', { updates: { sidebarCollapsed: next } }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  setWidth: (width: number) =>
    set({ width: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  collapse: () => {
    set({ isCollapsed: true })
    invoke('settings_update', { updates: { sidebarCollapsed: true } }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  expand: () => {
    set({ isCollapsed: false })
    invoke('settings_update', { updates: { sidebarCollapsed: false } }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  initFromSettings: async () => {
    try {
      const settings = await invoke<any>('settings_get')
      set({ isCollapsed: settings.sidebarCollapsed })
    } catch {
      // ignore — use defaults
    }
  }
}))

// Initialize on import
useSidebarStore.getState().initFromSettings()

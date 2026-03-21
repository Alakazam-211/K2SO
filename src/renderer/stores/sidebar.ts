import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { trpc } from '../lib/trpc'

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
    trpc.settings.update.mutate({ sidebarCollapsed: next }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  setWidth: (width: number) =>
    set({ width: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  collapse: () => {
    set({ isCollapsed: true })
    trpc.settings.update.mutate({ sidebarCollapsed: true }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  expand: () => {
    set({ isCollapsed: false })
    trpc.settings.update.mutate({ sidebarCollapsed: false }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  initFromSettings: async () => {
    try {
      const settings = await trpc.settings.get.query()
      set({ isCollapsed: settings.sidebarCollapsed })
    } catch {
      // ignore — use defaults
    }
  }
}))

// Initialize on import
useSidebarStore.getState().initFromSettings()

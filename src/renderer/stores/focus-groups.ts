import { create } from 'zustand'
import { trpc } from '@/lib/trpc'

export interface FocusGroup {
  id: string
  name: string
  color: string | null
  tabOrder: number
  createdAt: number
}

interface FocusGroupsState {
  focusGroups: FocusGroup[]
  activeFocusGroupId: string | null
  focusGroupsEnabled: boolean

  fetchFocusGroups: () => Promise<void>
  setActiveFocusGroup: (id: string | null) => void
  createFocusGroup: (name: string, color?: string) => Promise<void>
  deleteFocusGroup: (id: string) => Promise<void>
  renameFocusGroup: (id: string, name: string) => Promise<void>
  updateFocusGroupColor: (id: string, color: string | null) => Promise<void>
  assignProjectToGroup: (projectId: string, focusGroupId: string | null) => Promise<void>
  setFocusGroupsEnabled: (enabled: boolean) => Promise<void>
  initFromSettings: () => Promise<void>
}

export const useFocusGroupsStore = create<FocusGroupsState>((set, get) => ({
  focusGroups: [],
  activeFocusGroupId: null,
  focusGroupsEnabled: false,

  fetchFocusGroups: async () => {
    try {
      const groups = await trpc.focusGroups.list.query()
      set({ focusGroups: groups as FocusGroup[] })
    } catch (err) {
      console.error('[focus-groups] fetchFocusGroups failed:', err)
    }
  },

  setActiveFocusGroup: (id: string | null) => {
    set({ activeFocusGroupId: id })
  },

  createFocusGroup: async (name: string, color?: string) => {
    try {
      await trpc.focusGroups.create.mutate({ name, color })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] createFocusGroup failed:', err)
    }
  },

  deleteFocusGroup: async (id: string) => {
    try {
      const state = get()
      if (state.activeFocusGroupId === id) {
        set({ activeFocusGroupId: null })
      }
      await trpc.focusGroups.delete.mutate({ id })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] deleteFocusGroup failed:', err)
    }
  },

  renameFocusGroup: async (id: string, name: string) => {
    try {
      await trpc.focusGroups.update.mutate({ id, name })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] renameFocusGroup failed:', err)
    }
  },

  updateFocusGroupColor: async (id: string, color: string | null) => {
    try {
      await trpc.focusGroups.update.mutate({ id, color })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] updateFocusGroupColor failed:', err)
    }
  },

  assignProjectToGroup: async (projectId: string, focusGroupId: string | null) => {
    try {
      await trpc.focusGroups.assignProject.mutate({ projectId, focusGroupId })
    } catch (err) {
      console.error('[focus-groups] assignProjectToGroup failed:', err)
    }
  },

  setFocusGroupsEnabled: async (enabled: boolean) => {
    try {
      set({ focusGroupsEnabled: enabled })
      if (!enabled) {
        set({ activeFocusGroupId: null })
      }
      await trpc.settings.update.mutate({ focusGroupsEnabled: enabled })
    } catch (err) {
      console.error('[focus-groups] setFocusGroupsEnabled failed:', err)
    }
  },

  initFromSettings: async () => {
    try {
      const settings = await trpc.settings.get.query()
      set({ focusGroupsEnabled: settings.focusGroupsEnabled ?? false })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] initFromSettings failed:', err)
    }
  }
}))

// Initialize on import
useFocusGroupsStore.getState().initFromSettings()

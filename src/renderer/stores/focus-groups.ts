import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from './toast'

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
      const groups = await invoke<FocusGroup[]>('focus_groups_list')
      set({ focusGroups: groups })
    } catch (err) {
      console.error('[focus-groups] fetchFocusGroups failed:', err)
    }
  },

  setActiveFocusGroup: (id: string | null) => {
    set({ activeFocusGroupId: id })
  },

  createFocusGroup: async (name: string, color?: string) => {
    try {
      await invoke('focus_groups_create', { name, color })
      await get().fetchFocusGroups()
      useToastStore.getState().addToast('Focus group created', 'success')
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
      await invoke('focus_groups_delete', { id })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] deleteFocusGroup failed:', err)
    }
  },

  renameFocusGroup: async (id: string, name: string) => {
    try {
      await invoke('focus_groups_update', { id, name })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] renameFocusGroup failed:', err)
    }
  },

  updateFocusGroupColor: async (id: string, color: string | null) => {
    try {
      await invoke('focus_groups_update', { id, color })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] updateFocusGroupColor failed:', err)
    }
  },

  assignProjectToGroup: async (projectId: string, focusGroupId: string | null) => {
    try {
      await invoke('focus_groups_assign_project', { projectId, focusGroupId })
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
      await invoke('settings_update', { focusGroupsEnabled: enabled })
    } catch (err) {
      console.error('[focus-groups] setFocusGroupsEnabled failed:', err)
    }
  },

  initFromSettings: async () => {
    try {
      const settings = await invoke<any>('settings_get')
      set({ focusGroupsEnabled: settings.focusGroupsEnabled ?? false })
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] initFromSettings failed:', err)
    }
  }
}))

// Initialize on import
useFocusGroupsStore.getState().initFromSettings()

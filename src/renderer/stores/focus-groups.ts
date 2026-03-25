import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from './toast'
import { useProjectsStore } from './projects'

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
  reorderFocusGroups: (ids: string[]) => Promise<void>
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
    // Persist to settings so it restores on next launch
    if (id !== null) {
      invoke('settings_update', { updates: { activeFocusGroupId: id } }).catch(() => {})

      // Auto-activate the first workspace in the new focus group
      const projectsState = useProjectsStore.getState()
      const groupProjects = projectsState.projects.filter((p) => p.focusGroupId === id)
      if (groupProjects.length > 0) {
        const first = groupProjects[0]
        const firstWs = first.workspaces[0]
        if (firstWs) {
          projectsState.setActiveWorkspace(first.id, firstWs.id)
        }
      }
    }
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
        // Default to the first remaining group instead of null
        const remaining = state.focusGroups.filter((g) => g.id !== id)
        const nextId = remaining.length > 0 ? remaining[0].id : null
        set({ activeFocusGroupId: nextId })
        if (nextId) {
          invoke('settings_update', { updates: { activeFocusGroupId: nextId } }).catch(() => {})
        }
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

  reorderFocusGroups: async (ids: string[]) => {
    try {
      for (let i = 0; i < ids.length; i++) {
        await invoke('focus_groups_update', { id: ids[i], tabOrder: i })
      }
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] reorderFocusGroups failed:', err)
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
      if (enabled) {
        // Default to the first focus group, never "All Workspaces"
        const groups = get().focusGroups
        if (groups.length > 0 && !get().activeFocusGroupId) {
          const firstId = groups[0].id
          set({ activeFocusGroupId: firstId })
          invoke('settings_update', { updates: { activeFocusGroupId: firstId } }).catch(() => {})
        }
      } else {
        set({ activeFocusGroupId: null })
      }
      await invoke('settings_update', { updates: { focusGroupsEnabled: enabled } })
    } catch (err) {
      console.error('[focus-groups] setFocusGroupsEnabled failed:', err)
    }
  },

  initFromSettings: async () => {
    try {
      const settings = await invoke<any>('settings_get')
      const enabled = settings.focusGroupsEnabled ?? false
      set({ focusGroupsEnabled: enabled })
      await get().fetchFocusGroups()

      if (enabled) {
        const groups = get().focusGroups
        const savedId = settings.activeFocusGroupId as string | undefined
        // Restore saved group if it still exists, otherwise default to first group
        if (savedId && groups.some((g) => g.id === savedId)) {
          set({ activeFocusGroupId: savedId })
        } else if (groups.length > 0) {
          set({ activeFocusGroupId: groups[0].id })
        }
      }
    } catch (err) {
      console.error('[focus-groups] initFromSettings failed:', err)
    }
  }
}))

// Initialize on import
useFocusGroupsStore.getState().initFromSettings()

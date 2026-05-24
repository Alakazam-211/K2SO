import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
// Phase 2 Unit 7a — settings live in the daemon. `focus_groups_*`
// invokes still exist (Unit 4 territory); only the settings invokes
// move here.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
// Phase 2.5 fix (finding #547) — daemon-reconnect retry bus.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
import { useToastStore } from './toast'
import { useProjectsStore } from './projects'

/** Phase 2.5 fix (finding #547) — `hasLoadedFromDaemon` gate. See
 *  panels.ts for the rationale. Until the first successful
 *  `settingsGet()` returns, every persist call from this store is
 *  suppressed so the UI's optimistic defaults don't overwrite real
 *  settings during a brief boot-time daemon outage. */
let hasLoadedFromDaemon = false

function shouldSuppressPersist(): boolean {
  if (hasLoadedFromDaemon) return false
  console.warn(
    '[focus-groups] persist suppressed: settings not yet loaded from daemon'
  )
  return true
}

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
      if (!shouldSuppressPersist()) {
        settingsUpdate({ activeFocusGroupId: id }).catch((e) => console.warn('[focus-groups] settings_update failed:', e))
      }

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
        if (nextId && !shouldSuppressPersist()) {
          settingsUpdate({ activeFocusGroupId: nextId }).catch((e) => console.warn('[focus-groups] settings_update failed:', e))
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
          if (!shouldSuppressPersist()) {
            settingsUpdate({ activeFocusGroupId: firstId }).catch((e) => console.warn('[focus-groups] settings_update failed:', e))
          }
        }
      } else {
        set({ activeFocusGroupId: null })
      }
      if (shouldSuppressPersist()) return
      await settingsUpdate({ focusGroupsEnabled: enabled })
    } catch (err) {
      console.error('[focus-groups] setFocusGroupsEnabled failed:', err)
    }
  },

  initFromSettings: async () => {
    try {
      const settings = await settingsGet()
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
      // Phase 2.5 fix (finding #547): flip the persist gate ONLY
      // on a successful baseline fetch — never inside the catch.
      hasLoadedFromDaemon = true
    } catch (err) {
      console.error('[focus-groups] initFromSettings failed:', err)
    }
  }
}))

// Initialize on import
useFocusGroupsStore.getState().initFromSettings()

// Phase 2.5 fix (finding #547): retry on daemon (re)connect when
// the initial load hasn't yet succeeded. Steady-state no-op.
onDaemonConnected(() => {
  if (hasLoadedFromDaemon) return
  useFocusGroupsStore.getState().initFromSettings()
})

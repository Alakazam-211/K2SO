import { create } from 'zustand'
import { emit } from '@tauri-apps/api/event'
// Plan B — focus groups are host-aware daemon data: route them through
// the `/cli/focus-groups/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `focus_groups_*` invoke proxy. The old Tauri
// shims emitted `sync:focus-groups` (and `sync:projects` for the assign
// path) from Rust on each mutation; we re-emit those from the renderer
// after each successful mutation (see `emitFocusGroupsChanged`).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
// Phase 2 Unit 7a — settings live in the daemon.
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

/**
 * Plan B cross-window sync: the old Tauri `focus_groups_*` mutation
 * commands emitted `sync:focus-groups` from Rust so OTHER windows
 * re-fetch. The assign command additionally emitted `sync:projects`
 * (the project's focusGroupId changed). Now that the renderer talks to
 * the daemon directly, those Rust-side emits no longer fire — so we emit
 * the SAME events from the renderer after each successful mutation.
 * Fire-and-forget; a failed emit (non-Tauri/web) is non-fatal.
 */
function emitFocusGroupsChanged(): void {
  void emit('sync:focus-groups').catch((e) =>
    console.warn('[focus-groups] sync:focus-groups emit failed:', e),
  )
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
      // camelCase FocusGroup response matches the Rust struct's
      // `#[serde(rename_all = "camelCase")]`; no params on this GET.
      const groups = await daemonCliGet<FocusGroup[]>('focus-groups/list')
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
      await daemonCliPost('focus-groups/create', { name, color })
      emitFocusGroupsChanged()
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
      await daemonCliPost('focus-groups/delete', { id })
      emitFocusGroupsChanged()
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] deleteFocusGroup failed:', err)
    }
  },

  renameFocusGroup: async (id: string, name: string) => {
    try {
      await daemonCliPost('focus-groups/update', { id, name })
      emitFocusGroupsChanged()
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] renameFocusGroup failed:', err)
    }
  },

  updateFocusGroupColor: async (id: string, color: string | null) => {
    try {
      await daemonCliPost('focus-groups/update', { id, color })
      emitFocusGroupsChanged()
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] updateFocusGroupColor failed:', err)
    }
  },

  reorderFocusGroups: async (ids: string[]) => {
    try {
      for (let i = 0; i < ids.length; i++) {
        await daemonCliPost('focus-groups/update', { id: ids[i], tabOrder: i })
      }
      emitFocusGroupsChanged()
      await get().fetchFocusGroups()
    } catch (err) {
      console.error('[focus-groups] reorderFocusGroups failed:', err)
    }
  },

  assignProjectToGroup: async (projectId: string, focusGroupId: string | null) => {
    try {
      await daemonCliPost('focus-groups/assign', { projectId, focusGroupId })
      // The old Tauri `focus_groups_assign_project` emitted BOTH
      // `sync:focus-groups` and `sync:projects` (the project's
      // focusGroupId changed). Mirror both.
      emitFocusGroupsChanged()
      void emit('sync:projects').catch((e) =>
        console.warn('[focus-groups] sync:projects emit failed:', e),
      )
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

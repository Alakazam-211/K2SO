import { useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'
import { useTabsStore } from '@/stores/tabs'
import { useTimerStore } from '@/stores/timer'

/** True for the original main window only — label is exactly `main`.
 *  Focus windows are `focus-<id>`, "New Window" creates `window-<uuid>`.
 *  Non-main windows skip the mount-time `sync:tabs-request` broadcast:
 *  every window restores its own state from workspace_layouts, and
 *  adopting tabs from main duplicates them (the broadcast's tab IDs
 *  never match the restored tab IDs in applyRemoteTabChange). */
function isMainWindow(): boolean {
  if (window.location.hash.match(/^#focus=/)) return false
  try {
    return getCurrentWindow().label === 'main'
  } catch {
    return true
  }
}

/**
 * Listens for cross-window sync events emitted by the Rust backend.
 * When any window mutates persisted state (projects, settings, presets,
 * focus groups), the backend emits a sync event. All windows re-fetch
 * the relevant data to stay in sync.
 *
 * Also handles tab sync: on mount, requests existing tabs from other
 * windows. Responds to tab requests from newly opened windows.
 *
 * Call once in App.tsx — runs in every window instance.
 */
export function useWindowSync(): void {
  useEffect(() => {
    const unlisteners: Array<() => void> = []

    const setup = async (): Promise<void> => {
      unlisteners.push(
        await listen('sync:projects', () => {
          useProjectsStore.getState().fetchProjects()
        })
      )

      unlisteners.push(
        await listen('sync:settings', () => {
          useSettingsStore.getState().fetchSettings()
          usePanelsStore.getState().initFromSettings()
          useSidebarStore.getState().initFromSettings()
        })
      )

      unlisteners.push(
        await listen('sync:presets', () => {
          usePresetsStore.getState().fetchPresets()
        })
      )

      unlisteners.push(
        await listen('sync:focus-groups', () => {
          useFocusGroupsStore.getState().fetchFocusGroups()
        })
      )

      unlisteners.push(
        await listen<any>('sync:tabs', (event) => {
          useTabsStore.getState().applyRemoteTabChange(event.payload)
        })
      )

      unlisteners.push(
        await listen<any>('sync:timer', (event) => {
          useTimerStore.getState().syncFromEvent(event.payload)
        })
      )

      unlisteners.push(
        await listen('sync:timer-entries', () => {
          // Re-fetch entries if the timer settings section is open
          useTimerStore.getState().fetchEntries()
        })
      )

      // When another window asks for tabs, broadcast ours
      unlisteners.push(
        await listen('sync:tabs-request', () => {
          useTabsStore.getState().broadcastAllTabs()
        })
      )

      // Request existing tabs from other windows (slight delay so listeners are ready).
      //
      // Only the main window asks. Every non-main window (focus,
      // "New Window") restores its own state from workspace_layouts;
      // adopting tabs from a broadcast duplicates them because the
      // broadcast carries main's tab UUIDs which don't match the
      // restored tab UUIDs in applyRemoteTabChange. The runtime
      // sync:tabs listener stays active so add/remove/title events
      // still propagate live.
      //
      // 0.38.0: this is the primary leak that corrupts layout_json
      // over time. Until the daemon-authoritative refactor lands,
      // gating here is the cheapest fix.
      if (isMainWindow()) {
        setTimeout(() => {
          invoke('broadcast_sync', {
            channel: 'sync:tabs-request',
            payload: {},
          }).catch((e) => console.warn('[window-sync]', e))
        }, 500)
      }
    }

    setup()

    return () => {
      unlisteners.forEach((fn) => fn())
    }
  }, [])
}

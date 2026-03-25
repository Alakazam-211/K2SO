import { useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'
import { useTabsStore } from '@/stores/tabs'
import { useTimerStore } from '@/stores/timer'

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

      // Request existing tabs from other windows (slight delay so listeners are ready)
      setTimeout(() => {
        invoke('broadcast_sync', {
          channel: 'sync:tabs-request',
          payload: {},
        }).catch((e) => console.warn('[window-sync]', e))
      }, 500)
    }

    setup()

    return () => {
      unlisteners.forEach((fn) => fn())
    }
  }, [])
}

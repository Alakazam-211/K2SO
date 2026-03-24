import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'
import { useSettingsStore } from '@/stores/settings'
import { UPDATE_CHECK_INTERVAL } from '@shared/constants'

interface UpdateInfo {
  current_version: string
  latest_version: string
  download_url: string
  has_update: boolean
}

// Track latest known version to avoid repeat toasts for the same update
let lastNotifiedVersion: string | null = null

async function checkForUpdate(showToastIfNone = false): Promise<UpdateInfo | null> {
  try {
    const info = await invoke<UpdateInfo>('check_for_update')

    if (info.has_update && info.latest_version !== lastNotifiedVersion) {
      lastNotifiedVersion = info.latest_version
      useToastStore.getState().addToast(
        `K2SO v${info.latest_version} is available`,
        'info',
        8000,
        {
          label: 'Download',
          onClick: () => {
            // Open settings to general page and auto-trigger update check
            useSettingsStore.getState().openSettings('general')
            useSettingsStore.setState({ pendingUpdateCheck: true })
          },
        }
      )
    } else if (showToastIfNone && !info.has_update) {
      useToastStore.getState().addToast(
        `K2SO v${info.current_version} is up to date`,
        'success',
        3000
      )
    }

    return info
  } catch (err) {
    console.warn('[update-checker] Failed to check for updates:', err)
    return null
  }
}

/** Exported for Settings to trigger a manual check */
export { checkForUpdate }
export type { UpdateInfo }

/**
 * Hook that checks for updates on mount and every 3 hours.
 * Call once in App.tsx.
 */
export function useUpdateChecker(): void {
  useEffect(() => {
    // Check on launch (slight delay so the app settles first)
    const startupTimeout = setTimeout(() => checkForUpdate(), 5000)

    // Then every 3 hours
    const interval = setInterval(() => checkForUpdate(), UPDATE_CHECK_INTERVAL)

    return () => {
      clearTimeout(startupTimeout)
      clearInterval(interval)
    }
  }, [])
}

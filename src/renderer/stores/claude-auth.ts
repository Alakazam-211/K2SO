import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'

export type ClaudeAuthState = 'valid' | 'expiring' | 'expired' | 'missing' | 'unknown'

interface ClaudeAuthStatusResponse {
  state: string
  expiresAt: number | null
  secondsRemaining: number | null
  schedulerInstalled: boolean
}

interface ClaudeAuthStore {
  state: ClaudeAuthState
  expiresAt: number | null
  secondsRemaining: number | null
  schedulerInstalled: boolean
  refreshing: boolean
  lastError: string | null

  fetchStatus: () => Promise<void>
  refresh: () => Promise<void>
  installScheduler: () => Promise<void>
  uninstallScheduler: () => Promise<void>
}

export const useClaudeAuthStore = create<ClaudeAuthStore>((set) => ({
  state: 'unknown',
  expiresAt: null,
  secondsRemaining: null,
  schedulerInstalled: false,
  refreshing: false,
  lastError: null,

  fetchStatus: async () => {
    try {
      const status = await invoke<ClaudeAuthStatusResponse>('claude_auth_status')
      set({
        state: status.state as ClaudeAuthState,
        expiresAt: status.expiresAt,
        secondsRemaining: status.secondsRemaining,
        schedulerInstalled: status.schedulerInstalled,
        lastError: null,
      })
    } catch (e) {
      set({ lastError: String(e) })
    }
  },

  refresh: async () => {
    set({ refreshing: true, lastError: null })
    try {
      const status = await invoke<ClaudeAuthStatusResponse>('claude_auth_refresh')
      set({
        state: status.state as ClaudeAuthState,
        expiresAt: status.expiresAt,
        secondsRemaining: status.secondsRemaining,
        refreshing: false,
        lastError: null,
      })
    } catch (e) {
      set({ refreshing: false, lastError: String(e) })
    }
  },

  installScheduler: async () => {
    try {
      await invoke('claude_auth_install_scheduler')
      set({ schedulerInstalled: true, lastError: null })
    } catch (e) {
      set({ lastError: String(e) })
      throw e
    }
  },

  uninstallScheduler: async () => {
    try {
      await invoke('claude_auth_uninstall_scheduler')
      set({ schedulerInstalled: false, lastError: null })
    } catch (e) {
      set({ lastError: String(e) })
      throw e
    }
  },
}))

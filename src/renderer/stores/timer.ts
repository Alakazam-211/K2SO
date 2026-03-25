import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import type { AppSettingsResponse } from '@shared/types'

// ── Types ──────────────────────────────────────────────────────────────────

export interface TimeEntry {
  id: string
  projectId: string | null
  startTime: number    // UTC unix seconds
  endTime: number      // UTC unix seconds
  durationSeconds: number
  memo: string | null
  createdAt: number
}

export interface CountdownThemeConfig {
  name: string
  backgroundColor: string
  textColor: string
  fontFamily: string
  countdownTexts: string[]   // e.g. ["T-minus 3", "T-minus 2", "T-minus 1"]
  finalText: string          // e.g. "LIFTOFF!"
  animationPreset: 'fade' | 'zoom' | 'slide'
  flowTitles?: string[]      // shown when timer expires, e.g. ["You're on fire!", "Locked in."]
}

export type TimerStatus = 'idle' | 'running' | 'paused'

interface TimerState {
  // Runtime state (ephemeral, synced via events)
  status: TimerStatus
  startTime: number | null       // unix ms when timer started
  pausedElapsed: number          // accumulated ms before current resume
  resumeTime: number | null      // unix ms when last resumed
  targetDurationMs: number | null // countdown target in ms (null = count up)

  // Settings (from backend)
  visible: boolean
  countdownEnabled: boolean
  countdownTheme: string
  skipMemo: boolean
  timezone: string               // IANA timezone or '' for browser default
  customThemes: CountdownThemeConfig[]

  // UI state
  showCountdown: boolean
  showMemoDialog: boolean
  showExtendDialog: boolean
  stoppedElapsed: number | null  // ms elapsed at stop (for memo dialog display)

  // History
  entries: TimeEntry[]

  // Actions
  initFromSettings: () => Promise<void>
  beginCountdownOrStart: () => void
  startWithDuration: (durationMs: number) => void
  startTimer: () => void
  pauseTimer: () => void
  resumeTimer: () => void
  stopTimer: () => void
  stopTimerSilently: () => Promise<void>
  dismissCountdown: () => void
  cancelCountdown: () => void
  saveEntry: (memo?: string) => Promise<void>
  dismissMemoDialog: () => void
  showExtend: () => void
  extendTimer: (additionalMs: number) => void
  dismissExtendDialog: () => void

  // History actions
  fetchEntries: (start?: number, end?: number, projectId?: string) => Promise<void>
  deleteEntry: (id: string) => Promise<void>
  exportEntries: (format: 'csv' | 'json', start?: number, end?: number, projectId?: string) => Promise<string>

  // Settings actions
  updateTimerSetting: (key: string, value: any) => Promise<void>

  // Cross-window sync
  syncFromEvent: (payload: TimerSyncPayload) => void
}

export interface TimerSyncPayload {
  status: TimerStatus
  startTime: number | null
  pausedElapsed: number
  resumeTime: number | null
  targetDurationMs: number | null
}

function generateId(): string {
  return crypto.randomUUID()
}

function broadcastTimerState(state: TimerState): void {
  const payload: TimerSyncPayload = {
    status: state.status,
    startTime: state.startTime,
    pausedElapsed: state.pausedElapsed,
    resumeTime: state.resumeTime,
    targetDurationMs: state.targetDurationMs,
  }
  invoke('broadcast_sync', {
    channel: 'sync:timer',
    payload,
  }).catch((e) => console.warn('[timer] broadcast failed:', e))
}

/**
 * Get elapsed milliseconds from the timer state.
 */
export function getElapsedMs(state: { status: TimerStatus; pausedElapsed: number; resumeTime: number | null }): number {
  if (state.status === 'running' && state.resumeTime != null) {
    return state.pausedElapsed + (Date.now() - state.resumeTime)
  }
  return state.pausedElapsed
}

/**
 * Get remaining milliseconds for a countdown timer. Returns 0 if elapsed exceeds target.
 */
export function getRemainingMs(state: { status: TimerStatus; pausedElapsed: number; resumeTime: number | null; targetDurationMs: number | null }): number {
  if (state.targetDurationMs == null) return 0
  const elapsed = getElapsedMs(state)
  return Math.max(0, state.targetDurationMs - elapsed)
}

/**
 * Format milliseconds as H:MM:SS.
 */
export function formatElapsed(ms: number): string {
  const totalSec = Math.floor(ms / 1000)
  const h = Math.floor(totalSec / 3600)
  const m = Math.floor((totalSec % 3600) / 60)
  const s = totalSec % 60
  if (h > 0) {
    return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
  }
  return `${m}:${String(s).padStart(2, '0')}`
}

/**
 * Format a UTC unix-seconds timestamp for display in the given timezone.
 */
export function formatTimestamp(
  unixSeconds: number,
  timezone: string,
  options?: Intl.DateTimeFormatOptions
): string {
  const tz = timezone || Intl.DateTimeFormat().resolvedOptions().timeZone
  const date = new Date(unixSeconds * 1000)
  const defaults: Intl.DateTimeFormatOptions = {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    timeZone: tz,
  }
  return date.toLocaleString('en-US', { ...defaults, ...options })
}

/**
 * Format duration in seconds to a human-readable string.
 */
export function formatDuration(seconds: number): string {
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  const s = seconds % 60
  if (h > 0) return `${h}h ${m}m ${s}s`
  if (m > 0) return `${m}m ${s}s`
  return `${s}s`
}

export const useTimerStore = create<TimerState>((set, get) => ({
  // Runtime
  status: 'idle',
  startTime: null,
  pausedElapsed: 0,
  resumeTime: null,
  targetDurationMs: null,

  // Settings defaults
  visible: true,
  countdownEnabled: true,
  countdownTheme: 'rocket',
  skipMemo: false,
  timezone: '',
  customThemes: [],

  // UI
  showCountdown: false,
  showMemoDialog: false,
  showExtendDialog: false,
  stoppedElapsed: null,

  // History
  entries: [],

  initFromSettings: async () => {
    try {
      const result = await invoke<AppSettingsResponse>('settings_get')
      const timer = result.timer ?? {}
      set({
        visible: timer.visible ?? true,
        countdownEnabled: timer.countdownEnabled ?? true,
        countdownTheme: timer.countdownTheme ?? 'rocket',
        skipMemo: timer.skipMemo ?? false,
        timezone: timer.timezone ?? '',
        customThemes: (timer.customThemes ?? []) as unknown as CountdownThemeConfig[],
      })
    } catch (err) {
      console.error('[timer] Failed to load settings:', err)
    }
  },

  beginCountdownOrStart: () => {
    const state = get()
    if (state.status !== 'idle') return
    if (state.countdownEnabled) {
      set({ showCountdown: true })
    } else {
      get().startTimer()
    }
  },

  startWithDuration: (durationMs: number) => {
    const state = get()
    if (state.status !== 'idle') return
    set({ targetDurationMs: durationMs })
    if (state.countdownEnabled) {
      set({ showCountdown: true })
    } else {
      const now = Date.now()
      set({
        status: 'running',
        startTime: now,
        pausedElapsed: 0,
        resumeTime: now,
        showCountdown: false,
      })
      broadcastTimerState(get())
    }
  },

  startTimer: () => {
    const now = Date.now()
    set({
      status: 'running',
      startTime: now,
      pausedElapsed: 0,
      resumeTime: now,
      showCountdown: false,
    })
    broadcastTimerState(get())
  },

  pauseTimer: () => {
    const state = get()
    if (state.status !== 'running' || state.resumeTime == null) return
    const elapsed = state.pausedElapsed + (Date.now() - state.resumeTime)
    set({
      status: 'paused',
      pausedElapsed: elapsed,
      resumeTime: null,
    })
    broadcastTimerState(get())
  },

  resumeTimer: () => {
    const state = get()
    if (state.status !== 'paused') return
    set({
      status: 'running',
      resumeTime: Date.now(),
    })
    broadcastTimerState(get())
  },

  stopTimer: () => {
    const state = get()
    if (state.status === 'idle') return
    const elapsed = getElapsedMs(state)
    const { skipMemo } = state
    // Pause the timer to freeze the elapsed time
    set({
      status: 'paused',
      pausedElapsed: elapsed,
      resumeTime: null,
      stoppedElapsed: elapsed,
    })
    // Broadcast BEFORE showing memo dialog — broadcast_sync emits to all
    // windows including this one, and syncFromEvent resets showMemoDialog.
    // Use captured `skipMemo` (not get()) to avoid race with sync events.
    broadcastTimerState(get())
    if (skipMemo) {
      get().saveEntry()
    } else {
      set({ showMemoDialog: true })
    }
  },

  stopTimerSilently: async () => {
    const state = get()
    if (state.status === 'idle') return
    const elapsed = getElapsedMs(state)
    // Save entry immediately without memo dialog
    const startMs = state.startTime ?? Date.now()
    const durationSeconds = Math.round(elapsed / 1000)
    const startTimeSec = Math.floor(startMs / 1000)
    const endTimeSec = startTimeSec + durationSeconds
    const projectId = useProjectsStore.getState().activeProjectId ?? undefined

    try {
      await invoke('timer_entry_create', {
        id: generateId(),
        projectId: projectId ?? null,
        startTime: startTimeSec,
        endTime: endTimeSec,
        durationSeconds,
        memo: null,
      })
    } catch (err) {
      console.error('[timer] Failed to save entry on close:', err)
    }

    set({
      status: 'idle',
      startTime: null,
      pausedElapsed: 0,
      resumeTime: null,
      targetDurationMs: null,
      showMemoDialog: false,
      stoppedElapsed: null,
    })
  },

  dismissCountdown: () => {
    set({ showCountdown: false })
  },

  cancelCountdown: () => {
    set({ showCountdown: false })
  },

  saveEntry: async (memo?: string) => {
    const state = get()
    const startMs = state.startTime ?? Date.now()
    const elapsed = state.stoppedElapsed ?? state.pausedElapsed
    const durationSeconds = Math.round(elapsed / 1000)
    const startTimeSec = Math.floor(startMs / 1000)
    const endTimeSec = startTimeSec + durationSeconds
    const projectId = useProjectsStore.getState().activeProjectId ?? undefined

    const id = generateId()
    try {
      await invoke('timer_entry_create', {
        id,
        projectId: projectId ?? null,
        startTime: startTimeSec,
        endTime: endTimeSec,
        durationSeconds,
        memo: memo || null,
      })
    } catch (err) {
      console.error('[timer] Failed to save entry:', err)
      useToastStore.getState().addToast('Failed to save timer entry', 'error')
    }

    // Reset timer state (always reset — user shouldn't be stuck with a broken timer)
    set({
      status: 'idle',
      startTime: null,
      pausedElapsed: 0,
      resumeTime: null,
      targetDurationMs: null,
      showMemoDialog: false,
      stoppedElapsed: null,
    })
    broadcastTimerState(get())
  },

  dismissMemoDialog: () => {
    // Save without memo
    get().saveEntry()
  },

  showExtend: () => {
    const state = get()
    if (state.status !== 'running') return
    // Pause the timer while showing the extend dialog
    const elapsed = getElapsedMs(state)
    set({
      status: 'paused',
      pausedElapsed: elapsed,
      resumeTime: null,
      showExtendDialog: true,
    })
  },

  extendTimer: (additionalMs: number) => {
    const state = get()
    // Add more time to the target and resume
    const newTarget = (state.targetDurationMs ?? 0) + additionalMs
    set({
      status: 'running',
      targetDurationMs: newTarget,
      resumeTime: Date.now(),
      showExtendDialog: false,
    })
    broadcastTimerState(get())
  },

  dismissExtendDialog: () => {
    // User chose to stop — proceed to normal stop flow
    set({ showExtendDialog: false })
    get().stopTimer()
  },

  fetchEntries: async (start?: number, end?: number, projectId?: string) => {
    try {
      const entries = await invoke<TimeEntry[]>('timer_entries_list', {
        start: start ?? null,
        end: end ?? null,
        projectId: projectId ?? null,
      })
      set({ entries })
    } catch (err) {
      console.error('[timer] Failed to fetch entries:', err)
    }
  },

  deleteEntry: async (id: string) => {
    try {
      await invoke('timer_entry_delete', { id })
      // Remove locally
      set({ entries: get().entries.filter((e) => e.id !== id) })
    } catch (err) {
      console.error('[timer] Failed to delete entry:', err)
    }
  },

  exportEntries: async (format: 'csv' | 'json', start?: number, end?: number, projectId?: string) => {
    try {
      const result = await invoke<string>('timer_entries_export', {
        format,
        start: start ?? null,
        end: end ?? null,
        projectId: projectId ?? null,
      })
      return result
    } catch (err) {
      console.error('[timer] Failed to export:', err)
      return ''
    }
  },

  updateTimerSetting: async (key: string, value: any) => {
    // Optimistic update locally
    set({ [key]: value } as any)
    try {
      const currentSettings = await invoke<AppSettingsResponse>('settings_get')
      const timer = { ...(currentSettings.timer ?? {}), [key]: value }
      await invoke('settings_update', { updates: { timer } })
    } catch (err) {
      console.error('[timer] Failed to persist setting:', err)
    }
  },

  syncFromEvent: (payload: TimerSyncPayload) => {
    set({
      status: payload.status,
      startTime: payload.startTime,
      pausedElapsed: payload.pausedElapsed,
      resumeTime: payload.resumeTime,
      targetDurationMs: payload.targetDurationMs,
      // Close dialogs if synced from another window
      showCountdown: false,
      showMemoDialog: false,
      showExtendDialog: false,
    })
  },
}))

// Initialize on import
useTimerStore.getState().initFromSettings()

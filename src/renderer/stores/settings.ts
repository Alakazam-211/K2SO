import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { getDefaultKeybindings } from '@shared/hotkeys'
import type { AppSettingsResponse } from '@shared/types'

export type SettingsSection = 'general' | 'terminal' | 'editors-agents' | 'keybindings' | 'projects' | 'ai-assistant' | 'timer'

export interface TerminalSettings {
  fontFamily: string
  fontSize: number
  cursorStyle: 'bar' | 'block' | 'underline'
  scrollback: number
  naturalTextEditing: boolean
}

interface SettingsState {
  settingsOpen: boolean
  activeSection: SettingsSection

  // Terminal settings
  terminal: TerminalSettings

  // Keybindings: id -> key combo (overrides only; empty = use defaults)
  keybindings: Record<string, string>

  // Per-project settings
  projectSettings: Record<string, Record<string, any>>

  // AI Assistant
  aiAssistantEnabled: boolean

  // Master switch for the entire agent system (agents, pods, heartbeat, reviews)
  agenticSystemsEnabled: boolean

  // Claude Auth auto-refresh (background scheduler)
  claudeAuthAutoRefresh: boolean

  // Default agent CLI tool (e.g. 'claude', 'codex', 'gemini')
  defaultAgent: string

  // Pre-select a specific project in the projects section
  initialProjectId: string | null

  // Loading state
  loaded: boolean

  // When true, GeneralSection should auto-trigger an update check on mount
  pendingUpdateCheck: boolean

  // Actions
  openSettings: (section?: SettingsSection, projectId?: string) => void
  closeSettings: () => void
  setSection: (section: SettingsSection) => void
  updateTerminalSettings: (partial: Partial<TerminalSettings>) => void
  updateKeybinding: (id: string, combo: string) => void
  resetKeybinding: (id: string) => void
  resetAllKeybindings: () => void
  updateProjectSetting: (projectId: string, key: string, value: string) => void
  setAiAssistantEnabled: (enabled: boolean) => void
  setClaudeAuthAutoRefresh: (enabled: boolean) => void
  setDefaultAgent: (agent: string) => void
  resetAllSettings: () => void
  fetchSettings: () => Promise<void>
}

const DEFAULT_TERMINAL: TerminalSettings = {
  fontFamily: 'MesloLGM Nerd Font',
  fontSize: 13,
  cursorStyle: 'bar',
  scrollback: 5000,
  naturalTextEditing: true
}

/**
 * Monotonic write counter. Incremented each time we write settings.
 * fetchSettings() captures this before its async call and skips applying
 * the result if another write happened in the meantime (the write's result
 * is more authoritative than a stale read).
 */
let _writeSeq = 0

/**
 * Helper: sends a partial update to the backend, which deep-merges it
 * with the current settings on disk and returns the canonical result.
 * We apply the returned state to the store, ensuring we stay in sync
 * with what was actually persisted — no extra fetchSettings round-trip.
 */
async function persistAndApply(
  set: (state: Partial<SettingsState>) => void,
  updates: Record<string, any>
): Promise<void> {
  _writeSeq++
  try {
    const result = await invoke<AppSettingsResponse>('settings_update', { updates })
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? false,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      loaded: true
    })
  } catch (e) {
    console.warn('[settings] persistAndApply failed:', e)
  }
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settingsOpen: false,
  activeSection: 'general',
  terminal: { ...DEFAULT_TERMINAL },
  keybindings: {},
  projectSettings: {},
  aiAssistantEnabled: true,
  agenticSystemsEnabled: false,
  claudeAuthAutoRefresh: false,
  defaultAgent: 'claude',
  initialProjectId: null,
  loaded: false,
  pendingUpdateCheck: false,

  openSettings: (section?: SettingsSection, projectId?: string) => {
    set({
      settingsOpen: true,
      activeSection: section ?? get().activeSection,
      initialProjectId: projectId ?? null
    })
  },

  closeSettings: () => {
    set({ settingsOpen: false })
  },

  setSection: (section: SettingsSection) => {
    set({ activeSection: section })
  },

  updateTerminalSettings: async (partial: Partial<TerminalSettings>) => {
    const prev = get().terminal
    const newTerminal = { ...prev, ...partial }
    set({ terminal: newTerminal })
    try {
      await persistAndApply(set, { terminal: newTerminal })
    } catch (err) {
      console.error('[settings] Failed to persist terminal settings:', err)
      set({ terminal: prev })
    }
  },

  updateKeybinding: async (id: string, combo: string) => {
    const prev = get().keybindings
    const keybindings = { ...prev, [id]: combo }
    set({ keybindings })
    try {
      await persistAndApply(set, { keybindings })
    } catch (err) {
      console.error('[settings] Failed to persist keybinding:', err)
      set({ keybindings: prev })
    }
  },

  resetKeybinding: async (id: string) => {
    const prev = get().keybindings
    const keybindings = { ...prev }
    delete keybindings[id]
    set({ keybindings })
    try {
      await persistAndApply(set, { keybindings })
    } catch (err) {
      console.error('[settings] Failed to persist keybinding reset:', err)
      set({ keybindings: prev })
    }
  },

  resetAllKeybindings: async () => {
    const prev = get().keybindings
    set({ keybindings: {} })
    try {
      await persistAndApply(set, { keybindings: {} })
    } catch (err) {
      console.error('[settings] Failed to persist keybindings reset:', err)
      set({ keybindings: prev })
    }
  },

  updateProjectSetting: async (projectId: string, key: string, value: string) => {
    const prev = get().projectSettings
    const projectSettings = {
      ...prev,
      [projectId]: { ...prev[projectId], [key]: value }
    }
    set({ projectSettings })
    try {
      await persistAndApply(set, { projectSettings })
    } catch (err) {
      console.error('[settings] Failed to persist project setting:', err)
      set({ projectSettings: prev })
    }
  },

  setAiAssistantEnabled: async (enabled: boolean) => {
    const prev = get().aiAssistantEnabled
    set({ aiAssistantEnabled: enabled })
    try {
      await persistAndApply(set, { aiAssistantEnabled: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist AI assistant setting:', err)
      set({ aiAssistantEnabled: prev })
    }
  },

  setClaudeAuthAutoRefresh: async (enabled: boolean) => {
    const prev = get().claudeAuthAutoRefresh
    set({ claudeAuthAutoRefresh: enabled })
    try {
      await persistAndApply(set, { claudeAuthAutoRefresh: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist Claude auth auto-refresh setting:', err)
      set({ claudeAuthAutoRefresh: prev })
    }
  },

  setDefaultAgent: async (agent: string) => {
    const prev = get().defaultAgent
    set({ defaultAgent: agent })
    try {
      await persistAndApply(set, { defaultAgent: agent })
    } catch (err) {
      console.error('[settings] Failed to persist default agent:', err)
      set({ defaultAgent: prev })
    }
  },

  resetAllSettings: async () => {
    const result = await invoke<AppSettingsResponse>('settings_reset')
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {}
    })
  },

  fetchSettings: async () => {
    const seqBefore = _writeSeq
    const result = await invoke<AppSettingsResponse>('settings_get')
    // If a write happened while we were fetching, skip — the write's result is fresher
    if (_writeSeq !== seqBefore) return
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
      agenticSystemsEnabled: result.agenticSystemsEnabled ?? false,
      claudeAuthAutoRefresh: result.claudeAuthAutoRefresh ?? false,
      loaded: true
    })
  }
}))

/**
 * Get the effective key combo for a hotkey id,
 * falling back to the default if no override exists.
 */
export function getEffectiveKeybinding(
  keybindings: Record<string, string>,
  id: string
): string {
  if (keybindings[id]) return keybindings[id]
  const defaults = getDefaultKeybindings()
  return defaults[id] ?? ''
}

// Initialize on import
useSettingsStore.getState().fetchSettings()

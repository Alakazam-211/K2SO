import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { getDefaultKeybindings } from '@shared/hotkeys'

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
  projectSettings: Record<string, { defaultEditor?: string }>

  // AI Assistant
  aiAssistantEnabled: boolean

  // Default agent CLI tool (e.g. 'claude', 'codex', 'gemini')
  defaultAgent: string

  // Pre-select a specific project in the projects section
  initialProjectId: string | null

  // Loading state
  loaded: boolean

  // Actions
  openSettings: (section?: SettingsSection, projectId?: string) => void
  closeSettings: () => void
  setSection: (section: SettingsSection) => void
  updateTerminalSettings: (partial: Partial<TerminalSettings>) => void
  updateKeybinding: (id: string, combo: string) => void
  resetKeybinding: (id: string) => void
  resetAllKeybindings: () => void
  updateProjectSetting: (projectId: string, editor: string) => void
  setAiAssistantEnabled: (enabled: boolean) => void
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

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settingsOpen: false,
  activeSection: 'general',
  terminal: { ...DEFAULT_TERMINAL },
  keybindings: {},
  projectSettings: {},
  aiAssistantEnabled: true,
  defaultAgent: 'claude',
  initialProjectId: null,
  loaded: false,

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
      await invoke('settings_update', { terminal: newTerminal })
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
      await invoke('settings_update', { keybindings })
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
      await invoke('settings_update', { keybindings })
    } catch (err) {
      console.error('[settings] Failed to persist keybinding reset:', err)
      set({ keybindings: prev })
    }
  },

  resetAllKeybindings: async () => {
    const prev = get().keybindings
    set({ keybindings: {} })
    try {
      await invoke('settings_update', { keybindings: {} })
    } catch (err) {
      console.error('[settings] Failed to persist keybindings reset:', err)
      set({ keybindings: prev })
    }
  },

  updateProjectSetting: async (projectId: string, editor: string) => {
    const prev = get().projectSettings
    const projectSettings = {
      ...prev,
      [projectId]: { ...prev[projectId], defaultEditor: editor }
    }
    set({ projectSettings })
    try {
      await invoke('settings_update', { projectSettings })
    } catch (err) {
      console.error('[settings] Failed to persist project setting:', err)
      set({ projectSettings: prev })
    }
  },

  setAiAssistantEnabled: async (enabled: boolean) => {
    const prev = get().aiAssistantEnabled
    set({ aiAssistantEnabled: enabled })
    try {
      await invoke('settings_update', { aiAssistantEnabled: enabled })
    } catch (err) {
      console.error('[settings] Failed to persist AI assistant setting:', err)
      set({ aiAssistantEnabled: prev })
    }
  },

  setDefaultAgent: async (agent: string) => {
    const prev = get().defaultAgent
    set({ defaultAgent: agent })
    try {
      await invoke('settings_update', { defaultAgent: agent })
    } catch (err) {
      console.error('[settings] Failed to persist default agent:', err)
      set({ defaultAgent: prev })
    }
  },

  resetAllSettings: async () => {
    const result = await invoke<any>('settings_reset')
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {}
    })
  },

  fetchSettings: async () => {
    const result = await invoke<any>('settings_get')
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projectSettings ?? {},
      defaultAgent: result.defaultAgent ?? 'claude',
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

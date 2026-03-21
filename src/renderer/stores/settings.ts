import { create } from 'zustand'
import { trpc } from '@/lib/trpc'
import { getDefaultKeybindings } from '@shared/hotkeys'

export type SettingsSection = 'general' | 'terminal' | 'editors-agents' | 'keybindings' | 'projects'

export interface TerminalSettings {
  fontFamily: string
  fontSize: number
  cursorStyle: 'bar' | 'block' | 'underline'
  scrollback: number
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

  // Loading state
  loaded: boolean

  // Actions
  openSettings: (section?: SettingsSection) => void
  closeSettings: () => void
  setSection: (section: SettingsSection) => void
  updateTerminalSettings: (partial: Partial<TerminalSettings>) => void
  updateKeybinding: (id: string, combo: string) => void
  resetKeybinding: (id: string) => void
  resetAllKeybindings: () => void
  updateProjectSetting: (projectId: string, editor: string) => void
  resetAllSettings: () => void
  fetchSettings: () => Promise<void>
}

const DEFAULT_TERMINAL: TerminalSettings = {
  fontFamily: 'MesloLGM Nerd Font',
  fontSize: 13,
  cursorStyle: 'bar',
  scrollback: 5000
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settingsOpen: false,
  activeSection: 'general',
  terminal: { ...DEFAULT_TERMINAL },
  keybindings: {},
  projectSettings: {},
  loaded: false,

  openSettings: (section?: SettingsSection) => {
    set({ settingsOpen: true, activeSection: section ?? get().activeSection })
  },

  closeSettings: () => {
    set({ settingsOpen: false })
  },

  setSection: (section: SettingsSection) => {
    set({ activeSection: section })
  },

  updateTerminalSettings: async (partial: Partial<TerminalSettings>) => {
    const newTerminal = { ...get().terminal, ...partial }
    set({ terminal: newTerminal })
    await trpc.settings.update.mutate({ terminal: newTerminal })
  },

  updateKeybinding: async (id: string, combo: string) => {
    const keybindings = { ...get().keybindings, [id]: combo }
    set({ keybindings })
    await trpc.settings.update.mutate({ keybindings })
  },

  resetKeybinding: async (id: string) => {
    const keybindings = { ...get().keybindings }
    delete keybindings[id]
    set({ keybindings })
    await trpc.settings.update.mutate({ keybindings })
  },

  resetAllKeybindings: async () => {
    set({ keybindings: {} })
    await trpc.settings.update.mutate({ keybindings: {} })
  },

  updateProjectSetting: async (projectId: string, editor: string) => {
    const projectSettings = {
      ...get().projectSettings,
      [projectId]: { ...get().projectSettings[projectId], defaultEditor: editor }
    }
    set({ projectSettings })
    await trpc.settings.update.mutate({ projects: projectSettings })
  },

  resetAllSettings: async () => {
    const result = await trpc.settings.reset.mutate()
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projects
    })
  },

  fetchSettings: async () => {
    const result = await trpc.settings.get.query()
    set({
      terminal: result.terminal,
      keybindings: result.keybindings,
      projectSettings: result.projects,
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

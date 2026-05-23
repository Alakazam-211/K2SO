import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseCustomThemeJson, type ThemeColors } from '@/lib/editor-themes'
import type { HighlightStyle } from '@codemirror/language'

interface CustomTheme {
  id: string       // e.g. "custom:my-custom-theme"
  name: string     // Display name from JSON
  path: string     // Absolute file path
  colors: ThemeColors
  highlight: HighlightStyle
  isLight: boolean
}

interface CustomThemesStore {
  customThemes: CustomTheme[]
  creatorOpen: boolean
  activeThemePath: string | null

  loadCustomThemes: () => Promise<void>
  openCreator: (baseThemeJson: string) => Promise<string | null>
  closeCreator: () => void
  deleteCustomTheme: (id: string) => Promise<void>
  getTheme: (id: string) => CustomTheme | undefined
}

export const useCustomThemesStore = create<CustomThemesStore>((set, get) => ({
  customThemes: [],
  creatorOpen: false,
  activeThemePath: null,

  loadCustomThemes: async () => {
    try {
      const entries = await daemonCliGet<{ path: string; name: string; valid: boolean }[]>('themes/list')
      const themes: CustomTheme[] = []

      for (const entry of entries) {
        if (!entry.valid) continue
        try {
          const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: entry.path })
          const parsed = parseCustomThemeJson(result.content)
          if (!parsed) continue

          const id = `custom:${entry.path.split('/').pop()?.replace('.json', '') || entry.name}`
          themes.push({
            id,
            name: parsed.name,
            path: entry.path,
            colors: parsed.colors,
            highlight: parsed.highlight,
            isLight: parsed.type === 'light',
          })
        } catch {
          // Skip unreadable files
        }
      }

      set({ customThemes: themes })
    } catch (err) {
      console.warn('[custom-themes] Failed to load:', err)
    }
  },

  openCreator: async (baseThemeJson: string) => {
    try {
      const result = await daemonCliPost<{ path: string }>('themes/create-template', {
        base_theme_json: baseThemeJson,
      })
      const path = result.path
      set({ creatorOpen: true, activeThemePath: path })
      return path
    } catch (err) {
      console.error('[custom-themes] Failed to create template:', err)
      return null
    }
  },

  closeCreator: () => {
    set({ creatorOpen: false, activeThemePath: null })
    // Reload themes to pick up the new/edited one
    get().loadCustomThemes()
  },

  deleteCustomTheme: async (id: string) => {
    const theme = get().customThemes.find((t) => t.id === id)
    if (!theme) return
    try {
      await daemonCliPost('themes/delete', { path: theme.path })
      set({ customThemes: get().customThemes.filter((t) => t.id !== id) })
    } catch (err) {
      console.error('[custom-themes] Failed to delete:', err)
    }
  },

  getTheme: (id: string) => {
    return get().customThemes.find((t) => t.id === id)
  },
}))

import { create } from 'zustand'
import {
  TERMINAL_FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_DEFAULT
} from '../../shared/constants'

export type LinkClickMode = 'click' | 'cmd-click'

interface TerminalSettingsState {
  fontSize: number
  linkClickMode: LinkClickMode
  incrementFontSize: () => void
  decrementFontSize: () => void
  resetFontSize: () => void
  setLinkClickMode: (mode: LinkClickMode) => void
}

export const useTerminalSettingsStore = create<TerminalSettingsState>((set) => ({
  fontSize: TERMINAL_FONT_SIZE_DEFAULT,
  linkClickMode: 'click' as LinkClickMode,

  incrementFontSize: () => {
    set((state) => ({
      fontSize: Math.min(state.fontSize + 1, TERMINAL_FONT_SIZE_MAX)
    }))
  },

  decrementFontSize: () => {
    set((state) => ({
      fontSize: Math.max(state.fontSize - 1, TERMINAL_FONT_SIZE_MIN)
    }))
  },

  resetFontSize: () => {
    set({ fontSize: TERMINAL_FONT_SIZE_DEFAULT })
  },

  setLinkClickMode: (mode: LinkClickMode) => {
    set({ linkClickMode: mode })
  }
}))

// ── Wire up Tauri event listeners for zoom ──────────────────────────

async function initTerminalZoomListeners(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event')

    listen('terminal:zoom-in', () => {
      useTerminalSettingsStore.getState().incrementFontSize()
    })

    listen('terminal:zoom-out', () => {
      useTerminalSettingsStore.getState().decrementFontSize()
    })

    listen('terminal:zoom-reset', () => {
      useTerminalSettingsStore.getState().resetFontSize()
    })
  } catch {
    // Tauri API not available (e.g. in tests)
  }
}

// Initialize listeners on import
initTerminalZoomListeners()

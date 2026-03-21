import { create } from 'zustand'
import {
  TERMINAL_FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_DEFAULT
} from '../../shared/constants'

interface TerminalSettingsState {
  fontSize: number
  incrementFontSize: () => void
  decrementFontSize: () => void
  resetFontSize: () => void
}

export const useTerminalSettingsStore = create<TerminalSettingsState>((set) => ({
  fontSize: TERMINAL_FONT_SIZE_DEFAULT,

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
  }
}))

// ── Wire up IPC listeners from menu ──────────────────────────────────

function initTerminalZoomListeners(): void {
  const api = (window as any).api
  if (!api?.terminalZoom) return

  api.terminalZoom.onZoomIn(() => {
    useTerminalSettingsStore.getState().incrementFontSize()
  })

  api.terminalZoom.onZoomOut(() => {
    useTerminalSettingsStore.getState().decrementFontSize()
  })

  api.terminalZoom.onZoomReset(() => {
    useTerminalSettingsStore.getState().resetFontSize()
  })
}

// Initialize listeners on import
initTerminalZoomListeners()

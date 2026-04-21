import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import {
  TERMINAL_FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_DEFAULT
} from '../../shared/constants'

export type LinkClickMode = 'click' | 'cmd-click'
export type ShortcutModifierLayout = 'cmd-active-cmdshift-pinned' | 'cmd-pinned-cmdshift-active'

/**
 * Terminal rendering backend selection (Phase 4.5).
 *
 * - `alacritty` (default): the classic in-process alacritty_terminal
 *   engine + DOM renderer. Production-hardened, full SGR / alt-screen /
 *   mouse support. The current render path for every tab since K2SO
 *   shipped.
 * - `kessel`: subscribes to the daemon's Session Stream WebSocket
 *   (`/cli/sessions/subscribe`) and renders from Frame events. Beta —
 *   SGR parity as of 4.5, cursor blinks, but alt-screen buffer + full
 *   mouse reporting are not yet wired. Changes to this setting only
 *   affect NEW tabs; already-open tabs keep their chosen renderer.
 */
export type TerminalRenderer = 'alacritty' | 'kessel'

interface TerminalSettingsState {
  fontSize: number
  linkClickMode: LinkClickMode
  openLinksInSplitPane: boolean
  shortcutLayout: ShortcutModifierLayout
  renderer: TerminalRenderer
  incrementFontSize: () => void
  decrementFontSize: () => void
  resetFontSize: () => void
  setLinkClickMode: (mode: LinkClickMode) => void
  setOpenLinksInSplitPane: (enabled: boolean) => void
  setShortcutLayout: (layout: ShortcutModifierLayout) => void
  setRenderer: (renderer: TerminalRenderer) => void
}

// Persisted via zustand's persist middleware so the user's
// renderer + other preferences survive reload/restart. Prior to
// persistence, toggling to Kessel was silently lost on the next
// app launch — users would swap to Kessel, restart, see Alacritty,
// and assume the setting hadn't taken. Persisted in localStorage
// under the key below.
export const useTerminalSettingsStore = create<TerminalSettingsState>()(
  persist(
    (set) => ({
      fontSize: TERMINAL_FONT_SIZE_DEFAULT,
      linkClickMode: 'click' as LinkClickMode,
      openLinksInSplitPane: true,
      shortcutLayout: 'cmd-active-cmdshift-pinned' as ShortcutModifierLayout,
      // Default to alacritty while Kessel finishes baking. Users can opt
      // in per-preference; each new tab captures the preference at spawn
      // time so the choice doesn't hot-swap mid-session.
      renderer: 'alacritty' as TerminalRenderer,

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
      },

      setOpenLinksInSplitPane: (enabled: boolean) => {
        set({ openLinksInSplitPane: enabled })
      },

      setShortcutLayout: (layout: ShortcutModifierLayout) => {
        set({ shortcutLayout: layout })
      },

      setRenderer: (renderer: TerminalRenderer) => {
        set({ renderer })
      }
    }),
    {
      name: 'k2so-terminal-settings',
      storage: createJSONStorage(() => localStorage),
      // Persist only user-facing settings; never serialize the action
      // closures (they rebuild on load anyway).
      partialize: (state) => ({
        fontSize: state.fontSize,
        linkClickMode: state.linkClickMode,
        openLinksInSplitPane: state.openLinksInSplitPane,
        shortcutLayout: state.shortcutLayout,
        renderer: state.renderer,
      }),
      version: 1,
    },
  ),
)

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

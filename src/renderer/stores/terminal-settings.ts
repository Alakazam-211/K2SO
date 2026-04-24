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
 * Terminal rendering backend selection.
 *
 * - `alacritty` (default today): in-process alacritty_terminal engine +
 *   DOM renderer. PTY lives in the Tauri process; session dies with
 *   the app. Production-hardened, full SGR / alt-screen / mouse.
 *   Labeled "Alacritty (Legacy)" in the UI and retires after v2
 *   proves stable.
 * - `alacritty-v2`: daemon-hosted PTY + alacritty_terminal::Term.
 *   Tauri is a pure viewer rendering daemon-pushed grid snapshots +
 *   deltas. Session survives Tauri quit; heartbeats can target it.
 *   Labeled "Alacritty" in the UI. Tracks `.k2so/prds/alacritty-v2.md`
 *   phase plan; placeholder while A1-A5 land — selecting it currently
 *   falls back to `alacritty` behavior.
 * - `kessel`: experimental JSON-stream renderer for the six T1-capable
 *   CLI tools (Claude, Gemini, Cursor Agent, Codex, Goose, pi-mono).
 *   Multi-subscriber, per-device native reflow. Tracks
 *   `.k2so/prds/kessel-t1.md`. Labeled "Kessel (BETA)".
 *
 * Changes to this setting only affect NEW tabs; already-open tabs
 * keep their chosen renderer.
 */
export type TerminalRenderer = 'alacritty' | 'alacritty-v2' | 'kessel'

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

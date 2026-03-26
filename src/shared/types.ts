/** Shape of the trpc bridge exposed via contextBridge in the preload script */
export interface TrpcApi {
  invoke: (type: 'query' | 'mutation', path: string, input: unknown) => Promise<unknown>
  subscribe: (
    path: string,
    input: unknown,
    callbacks: {
      onData: (data: unknown) => void
      onError: (error: { message: string; code: string }) => void
      onComplete: () => void
    }
  ) => () => void
}

/** Context menu item shape */
export interface ContextMenuItem {
  id: string
  label: string
  type?: string
  enabled?: boolean
}

/** Terminal zoom IPC listeners */
export interface TerminalZoomApi {
  onZoomIn: (callback: () => void) => () => void
  onZoomOut: (callback: () => void) => () => void
  onZoomReset: (callback: () => void) => () => void
}

/** Full window.api shape exposed by the preload script */
export interface WindowApi {
  trpc: TrpcApi
  showContextMenu: (items: ContextMenuItem[]) => Promise<string | null>
  terminalZoom: TerminalZoomApi
}

/**
 * Re-export path for AppRouter type.
 * Import directly from the router module for type inference:
 *
 *   import type { AppRouter } from '../../main/lib/trpc/router'
 *
 * This file exists so renderer code can import shared interfaces
 * without pulling in main-process Node.js code at runtime.
 */
export type { WindowApi as WindowApiType, TrpcApi as TrpcApiType }

// ── Backend settings types (mirrors Rust AppSettings) ─────────────────

export interface TerminalSettingsBackend {
  fontFamily: string
  fontSize: number
  cursorStyle: 'bar' | 'block' | 'underline'
  scrollback: number
  naturalTextEditing: boolean
}

export interface TimerSettingsBackend {
  visible: boolean
  countdownEnabled: boolean
  countdownTheme: string
  skipMemo: boolean
  timezone: string
  customThemes: Record<string, string>[]
}

/** Matches Rust `AppSettings` (camelCase via serde rename) */
export interface AppSettingsResponse {
  terminal: TerminalSettingsBackend
  keybindings: Record<string, string>
  projectSettings: Record<string, Record<string, string>>
  focusGroupsEnabled: boolean
  activeFocusGroupId: string | null
  sidebarCollapsed: boolean
  leftPanelOpen: boolean
  rightPanelOpen: boolean
  leftPanelActiveTab: string
  rightPanelActiveTab: string
  leftPanelTabs: string[]
  rightPanelTabs: string[]
  defaultAgent: string
  aiAssistantEnabled: boolean
  timer: TimerSettingsBackend
  claudeAuthAutoRefresh: boolean
}

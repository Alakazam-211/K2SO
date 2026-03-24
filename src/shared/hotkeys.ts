// ── Hotkey configuration ──────────────────────────────────────────────

export interface HotkeyDefinition {
  id: string
  label: string
  defaultKey: string
  category: 'Terminal' | 'Tabs' | 'Navigation' | 'App'
}

/** Keys that cannot be rebound — they must pass through to the terminal */
export const RESERVED_KEYS = ['Ctrl+C', 'Ctrl+D', 'Ctrl+Z'] as const

export type ReservedKey = (typeof RESERVED_KEYS)[number]

export const HOTKEYS: HotkeyDefinition[] = [
  // ── Terminal ─────────────────────────────────────────────────────────
  { id: 'clearTerminal', label: 'Clear Terminal', defaultKey: 'Meta+K', category: 'Terminal' },
  {
    id: 'splitVertical',
    label: 'Split Pane Vertical',
    defaultKey: 'Meta+D',
    category: 'Terminal'
  },
  {
    id: 'splitHorizontal',
    label: 'Split Pane Horizontal',
    defaultKey: 'Meta+Shift+D',
    category: 'Terminal'
  },

  // ── Tabs ─────────────────────────────────────────────────────────────
  { id: 'newTab', label: 'New Tab', defaultKey: 'Meta+T', category: 'Tabs' },
  { id: 'closeTab', label: 'Close Tab', defaultKey: 'Meta+W', category: 'Tabs' },
  { id: 'prevTab', label: 'Previous Tab', defaultKey: 'Meta+Alt+ArrowLeft', category: 'Tabs' },
  { id: 'nextTab', label: 'Next Tab', defaultKey: 'Meta+Alt+ArrowRight', category: 'Tabs' },
  { id: 'workspace1', label: 'Switch to Workspace 1', defaultKey: 'Meta+1', category: 'Navigation' },
  { id: 'workspace2', label: 'Switch to Workspace 2', defaultKey: 'Meta+2', category: 'Navigation' },
  { id: 'workspace3', label: 'Switch to Workspace 3', defaultKey: 'Meta+3', category: 'Navigation' },
  { id: 'workspace4', label: 'Switch to Workspace 4', defaultKey: 'Meta+4', category: 'Navigation' },
  { id: 'workspace5', label: 'Switch to Workspace 5', defaultKey: 'Meta+5', category: 'Navigation' },
  { id: 'workspace6', label: 'Switch to Workspace 6', defaultKey: 'Meta+6', category: 'Navigation' },
  { id: 'workspace7', label: 'Switch to Workspace 7', defaultKey: 'Meta+7', category: 'Navigation' },
  { id: 'workspace8', label: 'Switch to Workspace 8', defaultKey: 'Meta+8', category: 'Navigation' },
  { id: 'workspace9', label: 'Switch to Workspace 9', defaultKey: 'Meta+9', category: 'Navigation' },

  // ── Navigation ──────────────────────────────────────────────────────
  {
    id: 'toggleSidebar',
    label: 'Toggle Sidebar',
    defaultKey: 'Meta+B',
    category: 'Navigation'
  },
  {
    id: 'toggleLeftPanel',
    label: 'Toggle Left Panel',
    defaultKey: 'Meta+Shift+E',
    category: 'Navigation'
  },
  {
    id: 'toggleRightPanel',
    label: 'Toggle Right Panel',
    defaultKey: 'Meta+Shift+B',
    category: 'Navigation'
  },

  // ── App ──────────────────────────────────────────────────────────────
  { id: 'openSettings', label: 'Open Settings', defaultKey: 'Meta+,', category: 'App' },
  { id: 'newWindow', label: 'New Window', defaultKey: 'Meta+Shift+N', category: 'App' },
  { id: 'newDocument', label: 'New Document', defaultKey: 'Meta+N', category: 'App' },
  { id: 'openWorkspace', label: 'Open Workspace', defaultKey: 'Meta+O', category: 'App' },
  { id: 'focusWindow', label: 'Open in Focus Window', defaultKey: 'Meta+Shift+F', category: 'App' },
  { id: 'toggleAssistant', label: 'Toggle Assistant', defaultKey: 'Meta+L', category: 'App' }
]

/** Build a map of id -> defaultKey for quick lookup */
export function getDefaultKeybindings(): Record<string, string> {
  const map: Record<string, string> = {}
  for (const h of HOTKEYS) {
    map[h.id] = h.defaultKey
  }
  return map
}

/**
 * Format a key combo string for display.
 * Converts Meta+ to Cmd+ on mac, Ctrl+ on other platforms.
 * NOTE: This function uses `navigator` and should only be called from the renderer.
 */
export function formatKeyCombo(combo: string, isMac = true): string {
  return combo
    .replace(/Meta\+/g, isMac ? '\u2318' : 'Ctrl+')
    .replace(/Shift\+/g, isMac ? '\u21E7' : 'Shift+')
    .replace(/Alt\+/g, isMac ? '\u2325' : 'Alt+')
    .replace(/Ctrl\+/g, isMac ? '\u2303' : 'Ctrl+')
    .replace(/ArrowLeft/g, '\u2190')
    .replace(/ArrowRight/g, '\u2192')
    .replace(/ArrowUp/g, '\u2191')
    .replace(/ArrowDown/g, '\u2193')
}

/** Minimal interface for the keyboard event fields we need */
interface KeyEventLike {
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  shiftKey: boolean
  key: string
}

/**
 * Convert a KeyboardEvent into a normalized key combo string
 * matching the format used in HOTKEYS definitions.
 */
export function keyEventToCombo(e: KeyEventLike): string {
  const parts: string[] = []
  if (e.ctrlKey) parts.push('Ctrl')
  if (e.metaKey) parts.push('Meta')
  if (e.altKey) parts.push('Alt')
  if (e.shiftKey) parts.push('Shift')

  // Avoid adding modifier keys themselves as the key part
  const ignoredKeys = new Set(['Control', 'Meta', 'Alt', 'Shift'])
  if (!ignoredKeys.has(e.key)) {
    // Normalize single character keys to uppercase for letters
    const key = e.key.length === 1 ? e.key.toUpperCase() : e.key
    parts.push(key)
  }

  return parts.join('+')
}

/**
 * Check if a key combo is reserved and cannot be rebound.
 */
export function isReservedKey(combo: string): boolean {
  return (RESERVED_KEYS as readonly string[]).includes(combo)
}

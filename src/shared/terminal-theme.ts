/**
 * Shared terminal color theme — VS Code Dark+ inspired.
 *
 * Used by both:
 *   - AlacrittyTerminalView (canvas renderer)
 *   - TerminalView (xterm.js theme)
 *
 * The Rust backend (alacritty_backend.rs) has a matching palette
 * in `default_color_palette()`. Keep them in sync.
 */

export const TERMINAL_THEME = {
  background: '#0a0a0a',
  foreground: '#e0e0e0',
  cursor: '#528bff',
  selectionBackground: '#264f78',

  // Standard 16 ANSI colors
  black: '#1e1e1e',
  red: '#f44747',
  green: '#6a9955',
  yellow: '#d7ba7d',
  blue: '#569cd6',
  magenta: '#c586c0',
  cyan: '#9cdcfe',
  white: '#d4d4d4',

  brightBlack: '#5a5a5a',
  brightRed: '#f44747',
  brightGreen: '#6a9955',
  brightYellow: '#d7ba7d',
  brightBlue: '#569cd6',
  brightMagenta: '#c586c0',
  brightCyan: '#9cdcfe',
  brightWhite: '#ffffff',
} as const

/**
 * Packed RGB values for canvas rendering (0xRRGGBB).
 */
export const TERMINAL_COLORS = {
  bg: 0x0a0a0a,
  fg: 0xe0e0e0,
  cursor: 0x528bff,
  selection: 0x264f78,
} as const

/**
 * Convert hex color string to packed RGB number.
 */
export function hexToPackedRGB(hex: string): number {
  const h = hex.replace('#', '')
  return parseInt(h, 16)
}

import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'

let cachedBackend: 'legacy' | 'alacritty' | null = null

/**
 * Returns the active terminal backend: 'legacy' (xterm.js) or 'alacritty'.
 * Result is cached after first call since it's determined at compile time.
 */
export function useTerminalBackend(): 'legacy' | 'alacritty' {
  const [backend, setBackend] = useState<'legacy' | 'alacritty'>(cachedBackend ?? 'alacritty')

  useEffect(() => {
    if (cachedBackend != null) return
    invoke<string>('terminal_get_backend').then((result) => {
      const value = result === 'alacritty' ? 'alacritty' : 'legacy'
      cachedBackend = value
      setBackend(value)
    }).catch(() => {
      cachedBackend = 'legacy'
    })
  }, [])

  return backend
}

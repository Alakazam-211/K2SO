import { BrowserWindow } from 'electron'
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'fs'
import { join } from 'path'
import { homedir, tmpdir } from 'os'

interface WindowState {
  x: number
  y: number
  width: number
  height: number
  isMaximized: boolean
}

const CONFIG_DIR = join(homedir(), '.k2so')
const STATE_FILE = join(CONFIG_DIR, 'window-state.json')

export function loadWindowState(): WindowState | null {
  try {
    if (!existsSync(STATE_FILE)) return null
    const raw = readFileSync(STATE_FILE, 'utf-8')
    return JSON.parse(raw) as WindowState
  } catch {
    return null
  }
}

function saveWindowState(state: WindowState): void {
  try {
    if (!existsSync(CONFIG_DIR)) {
      mkdirSync(CONFIG_DIR, { recursive: true })
    }
    // Atomic write: write to temp file then rename
    const tmpFile = join(tmpdir(), `k2so-window-state-${process.pid}.json`)
    writeFileSync(tmpFile, JSON.stringify(state, null, 2), 'utf-8')
    renameSync(tmpFile, STATE_FILE)
  } catch {
    // Silently ignore write errors — window state is not critical
  }
}

export function trackWindowState(win: BrowserWindow): void {
  let saveTimeout: ReturnType<typeof setTimeout> | null = null

  const debouncedSave = (): void => {
    if (saveTimeout) clearTimeout(saveTimeout)
    saveTimeout = setTimeout(() => {
      if (win.isDestroyed()) return
      try {
        const bounds = win.getBounds()
        saveWindowState({
          ...bounds,
          isMaximized: win.isMaximized()
        })
      } catch {
        // window may have been destroyed between check and access
      }
    }, 500)
  }

  const onClose = (): void => {
    if (saveTimeout) clearTimeout(saveTimeout)
    if (win.isDestroyed()) return
    try {
      const bounds = win.getBounds()
      saveWindowState({
        ...bounds,
        isMaximized: win.isMaximized()
      })
    } catch {
      // window may have been destroyed between check and access
    }
  }

  win.on('resize', debouncedSave)
  win.on('move', debouncedSave)
  win.on('close', onClose)

  // Clean up listeners when the window is fully closed
  win.on('closed', () => {
    if (saveTimeout) clearTimeout(saveTimeout)
    win.removeListener('resize', debouncedSave)
    win.removeListener('move', debouncedSave)
    win.removeListener('close', onClose)
  })
}

import { Tray, Menu, nativeImage, app } from 'electron'
import { createWindow, getWindows } from './window-manager'
import { terminalManager } from './terminal-manager'

let tray: Tray | null = null

/**
 * Create a 16x16 template icon programmatically.
 * Draws a simple "K" letter on a transparent background.
 */
function createTrayIcon(): Electron.NativeImage {
  // 16x16 RGBA buffer — draw a simple "K" shape in white
  const size = 16
  const buffer = Buffer.alloc(size * size * 4, 0) // transparent

  // Helper to set a pixel (white, fully opaque)
  const setPixel = (x: number, y: number): void => {
    if (x < 0 || x >= size || y < 0 || y >= size) return
    const offset = (y * size + x) * 4
    buffer[offset] = 255     // R
    buffer[offset + 1] = 255 // G
    buffer[offset + 2] = 255 // B
    buffer[offset + 3] = 255 // A
  }

  // Draw "K" — vertical bar on the left, two diagonals on the right
  for (let y = 2; y <= 13; y++) {
    // Vertical bar
    setPixel(3, y)
    setPixel(4, y)

    // Upper diagonal (going right-up from middle)
    const midY = 8
    if (y <= midY) {
      const dx = midY - y
      setPixel(5 + dx, y)
      setPixel(6 + dx, y)
    }
    // Lower diagonal (going right-down from middle)
    if (y >= midY) {
      const dx = y - midY
      setPixel(5 + dx, y)
      setPixel(6 + dx, y)
    }
  }

  const image = nativeImage.createFromBuffer(buffer, {
    width: size,
    height: size
  })

  image.setTemplateImage(true)
  return image
}

function buildContextMenu(): Menu {
  const activeCount = terminalManager.getActiveCount()

  return Menu.buildFromTemplate([
    {
      label: `${activeCount} Active Terminal${activeCount !== 1 ? 's' : ''}`,
      enabled: false
    },
    { type: 'separator' },
    {
      label: 'Show K2SO',
      click: (): void => {
        const windows = getWindows().filter(w => !w.isDestroyed())
        if (windows.length > 0) {
          const win = windows[0]
          if (win.isMinimized()) win.restore()
          win.focus()
        } else {
          createWindow()
        }
      }
    },
    {
      label: 'New Window',
      click: (): void => {
        createWindow()
      }
    },
    { type: 'separator' },
    {
      label: 'Quit K2SO',
      click: (): void => {
        app.quit()
      }
    }
  ])
}

export function setupTray(): void {
  const icon = createTrayIcon()
  tray = new Tray(icon)
  tray.setToolTip('K2SO')
  tray.setContextMenu(buildContextMenu())

  // Rebuild context menu on click to refresh terminal count
  tray.on('click', () => {
    if (tray) {
      tray.setContextMenu(buildContextMenu())
    }
  })
}

/**
 * Call this to refresh the tray menu (e.g., when terminals are created/destroyed).
 */
export function updateTrayMenu(): void {
  if (tray) {
    tray.setContextMenu(buildContextMenu())
  }
}

/**
 * Destroy the tray icon. Call on app quit.
 */
export function cleanupTray(): void {
  if (tray) {
    try {
      tray.destroy()
    } catch {
      // tray may already be destroyed
    }
    tray = null
  }
}

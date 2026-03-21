import { app, BrowserWindow, dialog, ipcMain, Menu, shell } from 'electron'
import { createMenu } from './lib/menu'
import { attachTrpcIpcHandler } from './lib/trpc'
import { createWindow, getWindows, setupWindowIpc, setupWindowAllClosed } from './lib/window-manager'
import { setupAutoUpdater, cleanupAutoUpdater, downloadUpdate, quitAndInstall } from './lib/auto-updater'
import { setupTray, cleanupTray } from './lib/tray'
import { setupDeepLinks } from './lib/deep-link'
import { terminalManager } from './lib/terminal-manager'

// Identify ourselves to child processes (terminals, etc.)
process.env.TERM_PROGRAM = 'K2SO'

// ── Single instance lock ──────────────────────────────────────────────
const gotTheLock = app.requestSingleInstanceLock()
if (!gotTheLock) {
  app.quit()
}

// ── Graceful close handling ───────────────────────────────────────────

let isQuitting = false

function confirmQuitWithActiveTerminals(): boolean {
  const activeCount = terminalManager.getActiveCount()
  if (activeCount === 0) return true

  const result = dialog.showMessageBoxSync({
    type: 'warning',
    title: 'Active Terminals Running',
    message: `${activeCount} active terminal${activeCount !== 1 ? 's are' : ' is'} still running.`,
    detail: 'Quitting will terminate all running terminals.',
    buttons: ['Cancel', 'Quit'],
    defaultId: 0,
    cancelId: 0
  })

  return result === 1
}

// ── External URL safety ──────────────────────────────────────────────

function setupExternalUrlSafety(win: BrowserWindow): void {
  if (win.isDestroyed()) return

  // Prevent the app from navigating away from the renderer
  win.webContents.on('will-navigate', (event, url) => {
    if (win.isDestroyed()) return
    // Allow dev server reloads
    if (!app.isPackaged && url.startsWith('http://localhost')) return

    // Block external navigation — open in system browser instead
    if (url.startsWith('http://') || url.startsWith('https://')) {
      event.preventDefault()
      shell.openExternal(url)
    }
  })

  // Intercept new window requests (target="_blank", window.open, etc.)
  if (!win.isDestroyed()) {
    win.webContents.setWindowOpenHandler(({ url }) => {
      if (url.startsWith('http://') || url.startsWith('https://')) {
        shell.openExternal(url)
      }
      return { action: 'deny' }
    })
  }
}

// ── Context menu IPC handler ──────────────────────────────────────────
ipcMain.handle(
  'context-menu:show',
  async (event, items: Array<{ id: string; label: string; type?: string; enabled?: boolean }>) => {
    return new Promise<string | null>((resolve) => {
      const senderWindow = BrowserWindow.fromWebContents(event.sender)
      if (!senderWindow) {
        resolve(null)
        return
      }

      const template = items.map((item) => {
        if (item.type === 'separator') {
          return { type: 'separator' as const }
        }
        return {
          label: item.label,
          enabled: item.enabled !== false,
          click: () => resolve(item.id)
        }
      })

      const menu = Menu.buildFromTemplate(template)
      menu.popup({
        window: senderWindow,
        callback: () => {
          // If no item was clicked, resolve null
          resolve(null)
        }
      })
    })
  }
)

// ── Auto-updater IPC handlers ────────────────────────────────────────
ipcMain.handle('updater:download', () => {
  downloadUpdate()
})

ipcMain.handle('updater:quit-and-install', () => {
  quitAndInstall()
})

// ── App lifecycle ────────────────────────────────────────────────────

app.on('before-quit', (event) => {
  if (isQuitting) return

  const shouldQuit = confirmQuitWithActiveTerminals()
  if (!shouldQuit) {
    event.preventDefault()
    return
  }

  isQuitting = true

  // Save state from all windows before quitting
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) {
      try {
        win.webContents.send('app:before-quit')
      } catch {
        // window may have been destroyed between check and send
      }
    }
  }

  terminalManager.killAll()
  cleanupTray()
  cleanupAutoUpdater()
})

app.whenReady().then(() => {
  attachTrpcIpcHandler()
  createMenu()
  setupWindowIpc()

  const win = createWindow()

  // Wire up external URL safety for all windows
  app.on('browser-window-created', (_event, newWin) => {
    if (newWin.isDestroyed()) return

    setupExternalUrlSafety(newWin)

    // Set up close confirmation
    newWin.on('close', (event) => {
      if (isQuitting) return
      if (newWin.isDestroyed()) return

      try {
        const allWindows = getWindows().filter(w => !w.isDestroyed())
        if (allWindows.length <= 1 && terminalManager.getActiveCount() > 0) {
          const shouldClose = confirmQuitWithActiveTerminals()
          if (!shouldClose) {
            event.preventDefault()
          }
        }
      } catch {
        // window may have been destroyed during check
      }
    })
  })

  // Apply to the initial window too
  if (!win.isDestroyed()) {
    setupExternalUrlSafety(win)
  }

  // Initialize distribution features
  setupTray()
  setupDeepLinks()

  // Only set up auto-updater in packaged builds
  if (app.isPackaged) {
    setupAutoUpdater()
  }

  app.on('activate', () => {
    // macOS: recreate window when dock icon is clicked and no windows exist
    const allWindows = BrowserWindow.getAllWindows().filter(w => !w.isDestroyed())
    if (allWindows.length === 0) {
      createWindow()
    }
  })
})

setupWindowAllClosed()

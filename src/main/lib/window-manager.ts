import { app, BrowserWindow, ipcMain, screen } from 'electron'
import { join } from 'path'
import { loadWindowState, trackWindowState } from './window-state'
import { attachTrpcIpcHandler, cleanupWindow } from './trpc'

const windows: BrowserWindow[] = []

/** Track focus windows by project ID to prevent duplicates */
const focusWindows = new Map<string, BrowserWindow>()

function getRendererUrl(): string | null {
  if (!app.isPackaged && process.env.ELECTRON_RENDERER_URL) {
    return process.env.ELECTRON_RENDERER_URL
  }
  return null
}

function getRendererFilePath(): string {
  return join(__dirname, '../renderer/index.html')
}

export function createWindow(): BrowserWindow {
  const savedState = loadWindowState()

  const win = new BrowserWindow({
    width: savedState?.width ?? 1400,
    height: savedState?.height ?? 900,
    x: savedState?.x,
    y: savedState?.y,
    minWidth: 800,
    minHeight: 600,
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 12, y: 12 },
    backgroundColor: '#0a0a0a',
    show: true,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: false, // Required for node-pty
      backgroundThrottling: false
    }
  })

  if (savedState?.isMaximized) {
    win.maximize()
  }

  trackWindowState(win)
  windows.push(win)

  // Clean up when window is closed
  win.on('closed', () => {
    cleanupWindow(win)
    const idx = windows.indexOf(win)
    if (idx !== -1) {
      windows.splice(idx, 1)
    }
  })

  // Load the renderer
  const devUrl = getRendererUrl()
  if (devUrl) {
    win.loadURL(devUrl)
  } else {
    win.loadFile(getRendererFilePath())
  }

  return win
}

export function createFocusWindow(projectId: string, projectName: string): BrowserWindow {
  // Deduplicate: if a focus window already exists for this project, focus it
  const existing = focusWindows.get(projectId)
  if (existing && !existing.isDestroyed()) {
    existing.focus()
    return existing
  }

  // Calculate cascading position offset from the main window (or screen center)
  const mainWin = BrowserWindow.getFocusedWindow() ?? windows[0]
  const display = mainWin
    ? screen.getDisplayMatching(mainWin.getBounds())
    : screen.getPrimaryDisplay()

  const CASCADE_OFFSET = 30

  let baseWidth: number
  let baseHeight: number
  let baseX: number
  let baseY: number

  if (mainWin && !mainWin.isDestroyed()) {
    const bounds = mainWin.getBounds()
    baseWidth = Math.round(bounds.width * 0.8)
    baseHeight = Math.round(bounds.height * 0.8)
    baseX = bounds.x + CASCADE_OFFSET
    baseY = bounds.y + CASCADE_OFFSET
  } else {
    baseWidth = Math.round(display.workAreaSize.width * 0.6)
    baseHeight = Math.round(display.workAreaSize.height * 0.7)
    baseX = display.workArea.x + Math.round((display.workAreaSize.width - baseWidth) / 2)
    baseY = display.workArea.y + Math.round((display.workAreaSize.height - baseHeight) / 2)
  }

  // Ensure the window stays on-screen
  const workArea = display.workArea
  if (baseX + baseWidth > workArea.x + workArea.width) {
    baseX = workArea.x + workArea.width - baseWidth
  }
  if (baseY + baseHeight > workArea.y + workArea.height) {
    baseY = workArea.y + workArea.height - baseHeight
  }
  if (baseX < workArea.x) baseX = workArea.x
  if (baseY < workArea.y) baseY = workArea.y

  const win = new BrowserWindow({
    width: Math.max(baseWidth, 600),
    height: Math.max(baseHeight, 400),
    x: baseX,
    y: baseY,
    minWidth: 600,
    minHeight: 400,
    title: projectName,
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 12, y: 12 },
    backgroundColor: '#0a0a0a',
    show: false,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: false,
      backgroundThrottling: false
    }
  })

  windows.push(win)
  focusWindows.set(projectId, win)

  win.on('closed', () => {
    cleanupWindow(win)
    const idx = windows.indexOf(win)
    if (idx !== -1) {
      windows.splice(idx, 1)
    }
    // Clean up focus window tracking
    if (focusWindows.get(projectId) === win) {
      focusWindows.delete(projectId)
    }
  })

  win.on('ready-to-show', () => {
    win.show()
  })

  const devUrl = getRendererUrl()
  const hash = `#focus=${encodeURIComponent(projectId)}`
  if (devUrl) {
    win.loadURL(`${devUrl}${hash}`)
  } else {
    win.loadFile(getRendererFilePath(), { hash })
  }

  return win
}

export function getWindows(): BrowserWindow[] {
  return [...windows]
}

export function getActiveWindow(): BrowserWindow | null {
  return BrowserWindow.getFocusedWindow()
}

export function closeWindow(id: number): void {
  const win = windows.find((w) => w.id === id)
  if (win && !win.isDestroyed()) {
    win.close()
  }
}

export function setupWindowIpc(): void {
  // Listen for "new window" requests from the renderer
  ipcMain.on('window:new', () => {
    createWindow()
  })
}

export function setupWindowAllClosed(): void {
  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') {
      app.quit()
    }
  })
}

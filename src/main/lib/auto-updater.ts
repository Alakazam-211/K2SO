import { autoUpdater, type UpdateInfo } from 'electron-updater'
import { BrowserWindow } from 'electron'
import { log } from './logger'

const CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000 // 4 hours
const INITIAL_DELAY_MS = 5000 // 5 seconds after launch

let checkIntervalId: ReturnType<typeof setInterval> | null = null
let initialTimeoutId: ReturnType<typeof setTimeout> | null = null

function sendToAllWindows(channel: string, ...args: unknown[]): void {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) {
      win.webContents.send(channel, ...args)
    }
  }
}

export function setupAutoUpdater(): void {
  // Don't auto-download — let the user choose
  autoUpdater.autoDownload = false
  autoUpdater.autoInstallOnAppQuit = true

  // ── Events ────────────────────────────────────────────────────────

  autoUpdater.on('checking-for-update', () => {
    log.info('[auto-updater] Checking for update...')
  })

  autoUpdater.on('update-available', (info: UpdateInfo) => {
    log.info(`[auto-updater] Update available: v${info.version}`)
    sendToAllWindows('updater:update-available', {
      version: info.version,
      releaseDate: info.releaseDate
    })
  })

  autoUpdater.on('update-not-available', (info: UpdateInfo) => {
    log.info(`[auto-updater] Already up to date: v${info.version}`)
  })

  autoUpdater.on('download-progress', (progress) => {
    log.info(`[auto-updater] Download progress: ${Math.round(progress.percent)}%`)
    sendToAllWindows('updater:download-progress', {
      percent: progress.percent,
      bytesPerSecond: progress.bytesPerSecond,
      transferred: progress.transferred,
      total: progress.total
    })
  })

  autoUpdater.on('update-downloaded', (info: UpdateInfo) => {
    log.info(`[auto-updater] Update downloaded: v${info.version}`)
    sendToAllWindows('updater:update-downloaded', {
      version: info.version,
      releaseDate: info.releaseDate
    })
  })

  autoUpdater.on('error', (err: Error) => {
    log.error('[auto-updater] Error:', err.message)
  })

  // ── Schedule checks ───────────────────────────────────────────────

  // Initial check after a short delay (let the app finish loading)
  initialTimeoutId = setTimeout(() => {
    initialTimeoutId = null
    autoUpdater.checkForUpdates().catch((err) => {
      log.error('[auto-updater] Initial check failed:', err.message)
    })
  }, INITIAL_DELAY_MS)

  // Periodic checks
  checkIntervalId = setInterval(() => {
    autoUpdater.checkForUpdates().catch((err) => {
      log.error('[auto-updater] Periodic check failed:', err.message)
    })
  }, CHECK_INTERVAL_MS)
}

/**
 * Clean up auto-updater timers. Call on app quit.
 */
export function cleanupAutoUpdater(): void {
  if (initialTimeoutId) {
    clearTimeout(initialTimeoutId)
    initialTimeoutId = null
  }
  if (checkIntervalId) {
    clearInterval(checkIntervalId)
    checkIntervalId = null
  }
}

/**
 * Trigger a download of the available update.
 * Call this from an IPC handler when the user clicks "Download".
 */
export function downloadUpdate(): void {
  autoUpdater.downloadUpdate().catch((err) => {
    log.error('[auto-updater] Download failed:', err.message)
  })
}

/**
 * Quit and install the downloaded update.
 */
export function quitAndInstall(): void {
  autoUpdater.quitAndInstall()
}

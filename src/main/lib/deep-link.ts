import { app } from 'electron'
import { createWindow, getWindows } from './window-manager'
import { log } from './logger'

// Buffer URLs received before the app is ready
const pendingUrls: string[] = []
let isReady = false

const PROTOCOL = 'k2so'

/**
 * Parse a k2so:// URL and act on it.
 *
 * Supported formats:
 *   k2so://open?path=/some/project
 */
function handleUrl(url: string): void {
  log.info(`[deep-link] Handling URL: ${url}`)

  try {
    const parsed = new URL(url)

    if (parsed.protocol !== `${PROTOCOL}:`) {
      log.warn(`[deep-link] Ignoring URL with unknown protocol: ${parsed.protocol}`)
      return
    }

    const action = parsed.hostname || parsed.pathname.replace(/^\/+/, '')

    switch (action) {
      case 'open': {
        const projectPath = parsed.searchParams.get('path')
        if (projectPath) {
          log.info(`[deep-link] Opening project: ${projectPath}`)
          // Find an existing window or create a new one
          const windows = getWindows()
          if (windows.length > 0) {
            const win = windows[0]
            if (win.isMinimized()) win.restore()
            win.focus()
            // Send the project path to the renderer
            win.webContents.send('deep-link:open-project', projectPath)
          } else {
            const win = createWindow()
            win.once('ready-to-show', () => {
              win.webContents.send('deep-link:open-project', projectPath)
            })
          }
        } else {
          log.warn('[deep-link] "open" action missing "path" parameter')
        }
        break
      }

      default:
        log.warn(`[deep-link] Unknown action: ${action}`)
    }
  } catch (err) {
    log.error(`[deep-link] Failed to parse URL: ${url}`, err)
  }
}

/**
 * Process any URLs that were queued before the app was ready.
 */
function flushPendingUrls(): void {
  isReady = true
  while (pendingUrls.length > 0) {
    const url = pendingUrls.shift()!
    handleUrl(url)
  }
}

export function setupDeepLinks(): void {
  // Register the protocol (macOS)
  if (!app.isDefaultProtocolClient(PROTOCOL)) {
    app.setAsDefaultProtocolClient(PROTOCOL)
  }

  // macOS: open-url event fires when a k2so:// URL is opened
  app.on('open-url', (event, url) => {
    event.preventDefault()
    if (isReady) {
      handleUrl(url)
    } else {
      pendingUrls.push(url)
    }
  })

  // All platforms: second-instance fires when another instance tries to launch
  app.on('second-instance', (_event, argv) => {
    // On macOS the URL comes via open-url, but on other platforms it's in argv
    const url = argv.find((arg) => arg.startsWith(`${PROTOCOL}://`))
    if (url) {
      if (isReady) {
        handleUrl(url)
      } else {
        pendingUrls.push(url)
      }
    }

    // Focus existing window
    const windows = getWindows()
    if (windows.length > 0) {
      const win = windows[0]
      if (win.isMinimized()) win.restore()
      win.focus()
    }
  })

  // Flush any buffered URLs now that the app is ready
  flushPendingUrls()
}

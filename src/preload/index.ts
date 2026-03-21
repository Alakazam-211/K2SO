import { contextBridge, ipcRenderer } from 'electron'
import type { TrpcApi } from '../shared/types'

// ── Generic IPC helpers ────────────────────────────────────────────────
const genericIpc = {
  /** Invoke a main-process handler and get a result back */
  invoke: (channel: string, ...args: unknown[]): Promise<unknown> => {
    return ipcRenderer.invoke(channel, ...args)
  },

  /** Listen for messages from the main process */
  on: (channel: string, callback: (...args: unknown[]) => void): void => {
    const listener = (_event: Electron.IpcRendererEvent, ...args: unknown[]): void => {
      callback(...args)
    }
    ipcRenderer.on(channel, listener)
  },

  /** Remove a listener */
  off: (channel: string, callback: (...args: unknown[]) => void): void => {
    ipcRenderer.removeListener(channel, callback as never)
  },

  /** Send a one-way message to the main process */
  send: (channel: string, ...args: unknown[]): void => {
    ipcRenderer.send(channel, ...args)
  }
}

// ── tRPC IPC bridge ────────────────────────────────────────────────────
const CHANNELS = {
  REQUEST: 'trpc:request',
  SUBSCRIBE: 'trpc:subscribe',
  SUBSCRIPTION_DATA: 'trpc:subscription-data',
  UNSUBSCRIBE: 'trpc:unsubscribe'
} as const

let subscriptionCounter = 0

const trpcApi: TrpcApi = {
  invoke: (type, path, input) => {
    return ipcRenderer.invoke(CHANNELS.REQUEST, { type, path, input })
  },

  subscribe: (path, input, callbacks) => {
    const subscriptionId = `sub_${++subscriptionCounter}_${Date.now()}`

    const handler = (
      _event: Electron.IpcRendererEvent,
      payload: {
        subscriptionId: string
        type: 'data' | 'error' | 'complete'
        data?: unknown
        error?: { message: string; code: string }
      }
    ): void => {
      if (payload.subscriptionId !== subscriptionId) return

      switch (payload.type) {
        case 'data':
          callbacks.onData(payload.data)
          break
        case 'error':
          callbacks.onError(payload.error!)
          break
        case 'complete':
          callbacks.onComplete()
          cleanup()
          break
      }
    }

    ipcRenderer.on(CHANNELS.SUBSCRIPTION_DATA, handler)
    ipcRenderer.send(CHANNELS.SUBSCRIBE, { subscriptionId, path, input })

    let cleaned = false
    const cleanup = (): void => {
      if (cleaned) return
      cleaned = true
      ipcRenderer.removeListener(CHANNELS.SUBSCRIPTION_DATA, handler)
      ipcRenderer.send(CHANNELS.UNSUBSCRIBE, { subscriptionId })
    }

    return cleanup
  }
}

// ── Context menu helper ───────────────────────────────────────────────
const contextMenu = {
  show: (
    items: Array<{ id: string; label: string; type?: string; enabled?: boolean }>
  ): Promise<string | null> => {
    return ipcRenderer.invoke('context-menu:show', items)
  }
}

// ── Terminal zoom IPC listeners ──────────────────────────────────────
const terminalZoom = {
  onZoomIn: (callback: () => void): (() => void) => {
    const listener = (): void => callback()
    ipcRenderer.on('terminal:zoom-in', listener)
    return () => ipcRenderer.removeListener('terminal:zoom-in', listener)
  },
  onZoomOut: (callback: () => void): (() => void) => {
    const listener = (): void => callback()
    ipcRenderer.on('terminal:zoom-out', listener)
    return () => ipcRenderer.removeListener('terminal:zoom-out', listener)
  },
  onZoomReset: (callback: () => void): (() => void) => {
    const listener = (): void => callback()
    ipcRenderer.on('terminal:zoom-reset', listener)
    return () => ipcRenderer.removeListener('terminal:zoom-reset', listener)
  }
}

// ── Expose combined API ────────────────────────────────────────────────
const api = {
  ...genericIpc,
  trpc: trpcApi,
  showContextMenu: contextMenu.show,
  terminalZoom
}

contextBridge.exposeInMainWorld('api', api)

export type K2SOApi = typeof api

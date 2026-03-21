import { ipcMain, BrowserWindow } from 'electron'
import { callTRPCProcedure, getTRPCErrorFromUnknown } from '@trpc/server'
import superjson from 'superjson'
import { appRouter } from './router'
import { createContext } from './trpc'
import type { ProcedureType } from '@trpc/server'
import type { Unsubscribable } from '@trpc/server/observable'

// ── Channel names ──────────────────────────────────────────────────────
const CHANNELS = {
  REQUEST: 'trpc:request',
  SUBSCRIBE: 'trpc:subscribe',
  SUBSCRIPTION_DATA: 'trpc:subscription-data',
  UNSUBSCRIBE: 'trpc:unsubscribe'
} as const

// ── Active subscription tracking ───────────────────────────────────────
// Key: `${webContentsId}:${subscriptionId}`
const activeSubscriptions = new Map<string, Unsubscribable>()

function subKey(webContentsId: number, subscriptionId: string): string {
  return `${webContentsId}:${subscriptionId}`
}

function cleanupSubscriptionsForWebContents(webContentsId: number): void {
  for (const [key, sub] of activeSubscriptions) {
    if (key.startsWith(`${webContentsId}:`)) {
      sub.unsubscribe()
      activeSubscriptions.delete(key)
    }
  }
}

// ── Singleton state ────────────────────────────────────────────────────
let isAttached = false

/**
 * Attach the tRPC IPC handlers. Safe to call multiple times (idempotent).
 */
export function attachTrpcIpcHandler(): void {
  if (isAttached) return
  isAttached = true

  // ── Query / Mutation (request-response) ──────────────────────────────
  ipcMain.handle(
    CHANNELS.REQUEST,
    async (event, payload: { type: 'query' | 'mutation'; path: string; input: unknown }) => {
      const senderWindow = BrowserWindow.fromWebContents(event.sender)
      if (!senderWindow) throw new Error('No BrowserWindow for sender')

      const { type, path, input } = payload

      try {
        const deserializedInput = input !== undefined ? superjson.deserialize(input as any) : undefined

        const result = await callTRPCProcedure({
          router: appRouter,
          path,
          getRawInput: async () => deserializedInput,
          ctx: createContext(senderWindow),
          type: type as ProcedureType,
          signal: undefined,
          batchIndex: 0
        })

        return { result: superjson.serialize(result) }
      } catch (cause) {
        const error = getTRPCErrorFromUnknown(cause)
        return {
          error: {
            message: error.message,
            code: error.code,
            data: { code: error.code, path }
          }
        }
      }
    }
  )

  // ── Subscriptions (long-lived push) ──────────────────────────────────
  ipcMain.on(
    CHANNELS.SUBSCRIBE,
    (event, payload: { subscriptionId: string; path: string; input: unknown }) => {
      const senderWindow = BrowserWindow.fromWebContents(event.sender)
      if (!senderWindow) return

      const { subscriptionId, path, input } = payload
      const key = subKey(event.sender.id, subscriptionId)

      // Prevent duplicate subscriptions
      if (activeSubscriptions.has(key)) return

      try {
        const deserializedInput = input !== undefined ? superjson.deserialize(input as any) : undefined

        const resultOrPromise = callTRPCProcedure({
          router: appRouter,
          path,
          getRawInput: async () => deserializedInput,
          ctx: createContext(senderWindow),
          type: 'subscription',
          signal: undefined,
          batchIndex: 0
        })

        // callProcedure for subscriptions returns an observable (or async iterable in v11)
        // We need to handle both cases
        const handleResult = (result: unknown): void => {
          if (result && typeof result === 'object' && 'subscribe' in result) {
            // Observable pattern
            const obs = result as { subscribe: (observer: any) => Unsubscribable }
            const subscription = obs.subscribe({
              next(data: unknown) {
                if (!event.sender.isDestroyed()) {
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'data',
                    data: superjson.serialize(data)
                  })
                }
              },
              error(err: unknown) {
                const trpcError = getTRPCErrorFromUnknown(err)
                if (!event.sender.isDestroyed()) {
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'error',
                    error: { message: trpcError.message, code: trpcError.code }
                  })
                }
                activeSubscriptions.delete(key)
              },
              complete() {
                if (!event.sender.isDestroyed()) {
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'complete'
                  })
                }
                activeSubscriptions.delete(key)
              }
            })
            activeSubscriptions.set(key, subscription)
          } else if (result && typeof result === 'object' && Symbol.asyncIterator in (result as any)) {
            // Async iterable pattern (tRPC v11 can return these)
            const iter = result as AsyncIterable<unknown>
            let cancelled = false
            const unsubscribable: Unsubscribable = {
              unsubscribe() {
                cancelled = true
              }
            }
            activeSubscriptions.set(key, unsubscribable)
            ;(async () => {
              try {
                for await (const data of iter) {
                  if (cancelled || event.sender.isDestroyed()) break
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'data',
                    data: superjson.serialize(data)
                  })
                }
                if (!cancelled && !event.sender.isDestroyed()) {
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'complete'
                  })
                }
              } catch (err) {
                const trpcError = getTRPCErrorFromUnknown(err)
                if (!cancelled && !event.sender.isDestroyed()) {
                  event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                    subscriptionId,
                    type: 'error',
                    error: { message: trpcError.message, code: trpcError.code }
                  })
                }
              } finally {
                activeSubscriptions.delete(key)
              }
            })()
          }
        }

        if (resultOrPromise instanceof Promise) {
          resultOrPromise.then(handleResult).catch((err) => {
            const trpcError = getTRPCErrorFromUnknown(err)
            if (!event.sender.isDestroyed()) {
              event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
                subscriptionId,
                type: 'error',
                error: { message: trpcError.message, code: trpcError.code }
              })
            }
          })
        } else {
          handleResult(resultOrPromise)
        }
      } catch (cause) {
        const error = getTRPCErrorFromUnknown(cause)
        if (!event.sender.isDestroyed()) {
          event.sender.send(CHANNELS.SUBSCRIPTION_DATA, {
            subscriptionId,
            type: 'error',
            error: { message: error.message, code: error.code }
          })
        }
      }
    }
  )

  // ── Unsubscribe ──────────────────────────────────────────────────────
  ipcMain.on(CHANNELS.UNSUBSCRIBE, (event, payload: { subscriptionId: string }) => {
    const key = subKey(event.sender.id, payload.subscriptionId)
    const sub = activeSubscriptions.get(key)
    if (sub) {
      sub.unsubscribe()
      activeSubscriptions.delete(key)
    }
  })
}

/**
 * Detach all tRPC IPC handlers and clean up active subscriptions.
 */
export function detachTrpcIpcHandler(): void {
  if (!isAttached) return
  isAttached = false

  ipcMain.removeHandler(CHANNELS.REQUEST)
  ipcMain.removeAllListeners(CHANNELS.SUBSCRIBE)
  ipcMain.removeAllListeners(CHANNELS.UNSUBSCRIBE)

  // Tear down all active subscriptions
  for (const [, sub] of activeSubscriptions) {
    sub.unsubscribe()
  }
  activeSubscriptions.clear()
}

/**
 * Call this when a BrowserWindow closes to clean up its subscriptions.
 */
export function cleanupWindow(window: BrowserWindow): void {
  cleanupSubscriptionsForWebContents(window.webContents.id)
}

import { createTRPCClient, TRPCClientError } from '@trpc/client'
import type { TRPCLink } from '@trpc/client'
import { observable } from '@trpc/server/observable'
import superjson from 'superjson'
import type { AppRouter } from '@shared/trpc'
import type { WindowApi } from '@shared/types'

// ── Augment the global Window type ─────────────────────────────────────
declare global {
  interface Window {
    api: WindowApi & {
      invoke: (channel: string, ...args: unknown[]) => Promise<unknown>
      on: (channel: string, callback: (...args: unknown[]) => void) => void
      off: (channel: string, callback: (...args: unknown[]) => void) => void
      send: (channel: string, ...args: unknown[]) => void
    }
  }
}

/**
 * Custom tRPC link that routes requests through Electron IPC
 * instead of HTTP. Handles query, mutation, and subscription types.
 */
function ipcLink(): TRPCLink<AppRouter> {
  return () =>
    ({ op }) => {
      // Subscriptions use the push-based channel
      if (op.type === 'subscription') {
        return observable((observer) => {
          const serializedInput =
            op.input !== undefined ? superjson.serialize(op.input) : undefined

          const unsubscribe = window.api.trpc.subscribe(op.path, serializedInput, {
            onData(data: unknown) {
              // Data arrives already superjson-serialized from main
              const deserialized = superjson.deserialize(data as any)
              observer.next({ result: { type: 'data', data: deserialized } })
            },
            onError(error) {
              observer.error(TRPCClientError.from(new Error(error.message)))
            },
            onComplete() {
              observer.complete()
            }
          })

          return () => {
            unsubscribe()
          }
        })
      }

      // Queries and mutations use request/response
      const opType = op.type as 'query' | 'mutation'
      return observable((observer) => {
        const serializedInput =
          op.input !== undefined ? superjson.serialize(op.input) : undefined

        window.api.trpc
          .invoke(opType, op.path, serializedInput)
          .then((response: any) => {
            if ('error' in response) {
              observer.error(TRPCClientError.from(new Error(response.error.message)))
            } else {
              const deserialized = superjson.deserialize(response.result)
              observer.next({ result: { type: 'data', data: deserialized } })
              observer.complete()
            }
          })
          .catch((err: unknown) => {
            observer.error(
              TRPCClientError.from(
                err instanceof Error ? err : new Error('Unknown IPC error')
              )
            )
          })

        // No abort mechanism for ipcRenderer.invoke
        return () => {}
      })
    }
}

/**
 * Fully typed tRPC client for the renderer process.
 *
 * Usage:
 *   const pong = await trpc.ping.query()
 */
export const trpc = createTRPCClient<AppRouter>({
  links: [ipcLink()]
})

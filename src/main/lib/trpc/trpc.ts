import { initTRPC } from '@trpc/server'
import superjson from 'superjson'
import type { BrowserWindow } from 'electron'

/**
 * Context available to every tRPC procedure.
 * `sender` is the BrowserWindow that originated the request,
 * enabling per-window state and targeted IPC responses.
 */
export interface Context {
  sender: BrowserWindow
}

export function createContext(sender: BrowserWindow): Context {
  return { sender }
}

const t = initTRPC.context<Context>().create({
  transformer: superjson
})

export const router = t.router
export const publicProcedure = t.procedure
export const createCallerFactory = t.createCallerFactory

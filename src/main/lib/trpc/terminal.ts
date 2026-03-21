import { randomUUID } from 'crypto'
import { z } from 'zod'
import { observable } from '@trpc/server/observable'
import { router, publicProcedure } from './trpc'
import { terminalManager } from '../terminal-manager'

export const terminalRouter = router({
  create: publicProcedure
    .input(
      z.object({
        cwd: z.string(),
        command: z.string().optional(),
        args: z.array(z.string()).optional()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()
      try {
        terminalManager.createTerminal({
          id,
          cwd: input.cwd,
          command: input.command,
          args: input.args
        })
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        console.error(`[terminal] Failed to create terminal: ${message}`)
        throw new Error(`Failed to create terminal: ${message}`)
      }
      return { id }
    }),

  write: publicProcedure
    .input(
      z.object({
        id: z.string(),
        data: z.string()
      })
    )
    .mutation(({ input }) => {
      terminalManager.writeToTerminal(input.id, input.data)
    }),

  resize: publicProcedure
    .input(
      z.object({
        id: z.string(),
        cols: z.number().int().positive(),
        rows: z.number().int().positive()
      })
    )
    .mutation(({ input }) => {
      terminalManager.resizeTerminal(input.id, input.cols, input.rows)
    }),

  kill: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      terminalManager.killTerminal(input.id)
    }),

  onData: publicProcedure
    .input(z.object({ id: z.string() }))
    .subscription(({ input }) => {
      return observable<string>((emit) => {
        const removeListener = terminalManager.onData(input.id, (data) => {
          emit.next(data)
        })

        return () => {
          removeListener()
        }
      })
    }),

  activeCountForPath: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(({ input }) => {
      return terminalManager.getTerminalCountForPath(input.path)
    }),

  onExit: publicProcedure
    .input(z.object({ id: z.string() }))
    .subscription(({ input }) => {
      return observable<{ exitCode: number; signal?: number }>((emit) => {
        const removeListener = terminalManager.onExit(input.id, (exitCode, signal) => {
          emit.next({ exitCode, signal })
          emit.complete()
        })

        return () => {
          removeListener()
        }
      })
    })
})

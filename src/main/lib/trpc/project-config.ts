import { z } from 'zod'
import { router, publicProcedure } from './trpc'
import {
  getProjectConfig,
  hasRunCommand
} from '../project-config'

export const projectConfigRouter = router({
  get: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(({ input }) => {
      return getProjectConfig(input.path)
    }),

  hasRunCommand: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(({ input }) => {
      return hasRunCommand(input.path)
    }),

  runCommand: publicProcedure
    .input(z.object({ path: z.string() }))
    .mutation(({ input }) => {
      const config = getProjectConfig(input.path)
      if (!config.runCommand) {
        throw new Error('No run command configured for this project')
      }
      return { command: config.runCommand }
    })
})

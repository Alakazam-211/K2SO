import { z } from 'zod'
import { readdirSync, statSync, readFileSync, accessSync, constants } from 'fs'
import { join, basename } from 'path'
import { shell, clipboard } from 'electron'
import { router, publicProcedure } from './trpc'

export const filesystemRouter = router({
  readDir: publicProcedure
    .input(
      z.object({
        path: z.string().min(1),
        showHidden: z.boolean().optional().default(false)
      })
    )
    .query(({ input }) => {
      try {
        const entries = readdirSync(input.path, { withFileTypes: true })

        const items = entries
          .filter((entry) => {
            if (!input.showHidden && entry.name.startsWith('.')) return false
            return true
          })
          .map((entry) => {
            const fullPath = join(input.path, entry.name)
            const isDirectory = entry.isDirectory()

            let size = 0
            let modifiedAt = 0

            try {
              const stat = statSync(fullPath)
              size = stat.size
              modifiedAt = stat.mtimeMs
            } catch {
              // Permission denied or broken symlink — use defaults
            }

            return {
              name: entry.name,
              path: fullPath,
              isDirectory,
              size,
              modifiedAt
            }
          })

        // Sort: directories first, then alphabetically (case-insensitive)
        items.sort((a, b) => {
          if (a.isDirectory !== b.isDirectory) {
            return a.isDirectory ? -1 : 1
          }
          return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })
        })

        return items
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to read directory'
        throw new Error(message)
      }
    }),

  openInFinder: publicProcedure
    .input(z.object({ path: z.string().min(1) }))
    .mutation(({ input }) => {
      shell.showItemInFolder(input.path)
      return { success: true }
    }),

  copyPath: publicProcedure
    .input(z.object({ path: z.string().min(1) }))
    .mutation(({ input }) => {
      clipboard.writeText(input.path)
      return { success: true }
    }),

  readFile: publicProcedure
    .input(z.object({ path: z.string().min(1) }))
    .query(({ input }) => {
      try {
        // Check file exists and is readable
        accessSync(input.path, constants.R_OK)

        const stat = statSync(input.path)

        // Reject files larger than 10MB to avoid memory issues
        if (stat.size > 10 * 1024 * 1024) {
          throw new Error('File too large (>10MB)')
        }

        // Read as buffer first to detect binary content
        const buffer = readFileSync(input.path)

        // Check for null bytes (binary file indicator)
        for (let i = 0; i < Math.min(buffer.length, 8192); i++) {
          if (buffer[i] === 0) {
            throw new Error('Cannot read binary file')
          }
        }

        const content = buffer.toString('utf-8')
        const name = basename(input.path)

        return { content, path: input.path, name }
      } catch (err) {
        if (err instanceof Error) {
          if ((err as NodeJS.ErrnoException).code === 'ENOENT') {
            throw new Error('File not found')
          }
          if ((err as NodeJS.ErrnoException).code === 'EACCES') {
            throw new Error('Permission denied')
          }
          throw err
        }
        throw new Error('Failed to read file')
      }
    })
})

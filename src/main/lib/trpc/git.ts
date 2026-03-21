import { z } from 'zod'
import { eq } from 'drizzle-orm'
import { randomUUID } from 'crypto'
import { router, publicProcedure } from './trpc'
import { db } from '../db'
import { workspaces } from '../db/schema'
import {
  getGitInfo,
  listBranches,
  listWorktrees,
  createWorktree,
  removeWorktree,
  getChangedFiles
} from '../git'

export const gitRouter = router({
  info: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(async ({ input }) => {
      return getGitInfo(input.path)
    }),

  branches: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(async ({ input }) => {
      return listBranches(input.path)
    }),

  worktrees: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(async ({ input }) => {
      return listWorktrees(input.path)
    }),

  createWorktree: publicProcedure
    .input(
      z.object({
        projectPath: z.string(),
        branch: z.string().min(1),
        newBranch: z.boolean().optional(),
        projectId: z.string().optional()
      })
    )
    .mutation(async ({ input }) => {
      const result = await createWorktree(input.projectPath, input.branch, input.newBranch)

      // If projectId is provided, create a workspace record
      if (input.projectId) {
        const id = randomUUID()

        // Get max tabOrder for the project
        const existing = db
          .select()
          .from(workspaces)
          .where(eq(workspaces.projectId, input.projectId))
          .all()
        const maxOrder =
          existing.length > 0 ? Math.max(...existing.map((w) => w.tabOrder)) + 1 : 0

        db.insert(workspaces)
          .values({
            id,
            projectId: input.projectId,
            name: input.branch,
            type: 'worktree',
            branch: input.branch,
            worktreePath: result.path,
            tabOrder: maxOrder
          })
          .run()

        return db.select().from(workspaces).where(eq(workspaces.id, id)).get()!
      }

      return { path: result.path, branch: result.branch }
    }),

  removeWorktree: publicProcedure
    .input(
      z.object({
        worktreePath: z.string(),
        projectPath: z.string().optional(),
        workspaceId: z.string().optional(),
        force: z.boolean().optional()
      })
    )
    .mutation(async ({ input }) => {
      // We need a project path to run git commands against.
      // Try to find it from the workspace record if not provided directly.
      let projectPath = input.projectPath
      if (!projectPath && input.workspaceId) {
        const workspace = db
          .select()
          .from(workspaces)
          .where(eq(workspaces.id, input.workspaceId))
          .get()
        if (workspace) {
          // The worktree's git dir points back to the main repo;
          // use the worktreePath's parent structure to derive it.
          // Alternatively, we stored projectPath elsewhere. For robustness,
          // we'll require projectPath in caller or skip git removal.
        }
      }

      if (projectPath) {
        await removeWorktree(projectPath, input.worktreePath, input.force)
      }

      // Delete workspace record from DB
      if (input.workspaceId) {
        db.delete(workspaces).where(eq(workspaces.id, input.workspaceId)).run()
      }

      return { success: true }
    }),

  reopenWorktree: publicProcedure
    .input(
      z.object({
        projectId: z.string(),
        worktreePath: z.string(),
        branch: z.string()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()

      // Get max tabOrder for the project
      const existing = db
        .select()
        .from(workspaces)
        .where(eq(workspaces.projectId, input.projectId))
        .all()
      const maxOrder =
        existing.length > 0 ? Math.max(...existing.map((w) => w.tabOrder)) + 1 : 0

      db.insert(workspaces)
        .values({
          id,
          projectId: input.projectId,
          name: input.branch,
          type: 'worktree',
          branch: input.branch,
          worktreePath: input.worktreePath,
          tabOrder: maxOrder
        })
        .run()

      return db.select().from(workspaces).where(eq(workspaces.id, id)).get()!
    }),

  changes: publicProcedure
    .input(z.object({ path: z.string() }))
    .query(async ({ input }) => {
      return getChangedFiles(input.path)
    })
})

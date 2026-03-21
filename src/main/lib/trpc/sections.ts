import { z } from 'zod'
import { eq, asc } from 'drizzle-orm'
import { randomUUID } from 'crypto'
import { router, publicProcedure } from './trpc'
import { db } from '../db'
import { workspaceSections, workspaces } from '../db/schema'

export const sectionsRouter = router({
  list: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .query(({ input }) => {
      return db
        .select()
        .from(workspaceSections)
        .where(eq(workspaceSections.projectId, input.projectId))
        .orderBy(asc(workspaceSections.tabOrder))
        .all()
    }),

  create: publicProcedure
    .input(
      z.object({
        projectId: z.string(),
        name: z.string().min(1),
        color: z.string().optional()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()

      const existing = db
        .select()
        .from(workspaceSections)
        .where(eq(workspaceSections.projectId, input.projectId))
        .all()
      const maxOrder =
        existing.length > 0 ? Math.max(...existing.map((s) => s.tabOrder)) + 1 : 0

      db.insert(workspaceSections)
        .values({
          id,
          projectId: input.projectId,
          name: input.name,
          color: input.color ?? null,
          tabOrder: maxOrder
        })
        .run()

      return db.select().from(workspaceSections).where(eq(workspaceSections.id, id)).get()!
    }),

  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        name: z.string().optional(),
        color: z.string().nullable().optional(),
        isCollapsed: z.number().optional(),
        tabOrder: z.number().optional()
      })
    )
    .mutation(({ input }) => {
      const { id, ...updates } = input
      const setValues: Record<string, unknown> = {}
      if (updates.name !== undefined) setValues.name = updates.name
      if (updates.color !== undefined) setValues.color = updates.color
      if (updates.isCollapsed !== undefined) setValues.isCollapsed = updates.isCollapsed
      if (updates.tabOrder !== undefined) setValues.tabOrder = updates.tabOrder

      if (Object.keys(setValues).length > 0) {
        db.update(workspaceSections).set(setValues).where(eq(workspaceSections.id, id)).run()
      }

      return db.select().from(workspaceSections).where(eq(workspaceSections.id, id)).get()!
    }),

  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      db.delete(workspaceSections).where(eq(workspaceSections.id, input.id)).run()
      return { success: true }
    }),

  reorder: publicProcedure
    .input(z.object({ ids: z.array(z.string()) }))
    .mutation(({ input }) => {
      for (let i = 0; i < input.ids.length; i++) {
        db.update(workspaceSections)
          .set({ tabOrder: i })
          .where(eq(workspaceSections.id, input.ids[i]))
          .run()
      }
      return { success: true }
    }),

  assignWorkspace: publicProcedure
    .input(
      z.object({
        workspaceId: z.string(),
        sectionId: z.string().nullable()
      })
    )
    .mutation(({ input }) => {
      db.update(workspaces)
        .set({ sectionId: input.sectionId })
        .where(eq(workspaces.id, input.workspaceId))
        .run()
      return db.select().from(workspaces).where(eq(workspaces.id, input.workspaceId)).get()!
    })
})

import { z } from 'zod'
import { eq, asc } from 'drizzle-orm'
import { randomUUID } from 'crypto'
import { router, publicProcedure } from './trpc'
import { db } from '../db'
import { focusGroups, projects } from '../db/schema'
import { setProjectConfigValue, getProjectConfig } from '../project-config'

export const focusGroupsRouter = router({
  list: publicProcedure.query(() => {
    return db.select().from(focusGroups).orderBy(asc(focusGroups.tabOrder)).all()
  }),

  create: publicProcedure
    .input(
      z.object({
        name: z.string().min(1),
        color: z.string().optional()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()

      const existing = db.select().from(focusGroups).orderBy(asc(focusGroups.tabOrder)).all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((g) => g.tabOrder)) + 1 : 0

      db.insert(focusGroups)
        .values({
          id,
          name: input.name,
          color: input.color ?? null,
          tabOrder: maxOrder
        })
        .run()

      return db.select().from(focusGroups).where(eq(focusGroups.id, id)).get()!
    }),

  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        name: z.string().optional(),
        color: z.string().nullable().optional(),
        tabOrder: z.number().optional()
      })
    )
    .mutation(({ input }) => {
      const { id, ...updates } = input
      const setValues: Record<string, unknown> = {}
      if (updates.name !== undefined) setValues.name = updates.name
      if (updates.color !== undefined) setValues.color = updates.color
      if (updates.tabOrder !== undefined) setValues.tabOrder = updates.tabOrder

      if (Object.keys(setValues).length > 0) {
        db.update(focusGroups).set(setValues).where(eq(focusGroups.id, id)).run()
      }

      return db.select().from(focusGroups).where(eq(focusGroups.id, id)).get()!
    }),

  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      db.delete(focusGroups).where(eq(focusGroups.id, input.id)).run()
      return { success: true }
    }),

  assignProject: publicProcedure
    .input(
      z.object({
        projectId: z.string(),
        focusGroupId: z.string().nullable()
      })
    )
    .mutation(({ input }) => {
      db.update(projects)
        .set({ focusGroupId: input.focusGroupId })
        .where(eq(projects.id, input.projectId))
        .run()

      // Write the focus group name to .k2so/config.json
      const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
      if (project) {
        let groupName: string | null = null
        if (input.focusGroupId) {
          const group = db
            .select()
            .from(focusGroups)
            .where(eq(focusGroups.id, input.focusGroupId))
            .get()
          groupName = group?.name ?? null
        }
        setProjectConfigValue(project.path, 'focusGroupName', groupName)
      }

      return db.select().from(projects).where(eq(projects.id, input.projectId)).get()!
    }),

  reconcileProject: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .mutation(({ input }) => {
      const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
      if (!project) {
        throw new Error('Project not found')
      }

      const config = getProjectConfig(project.path)
      const configGroupName = config.focusGroupName

      if (!configGroupName) {
        // Config says no group — clear the DB if needed
        if (project.focusGroupId) {
          db.update(projects)
            .set({ focusGroupId: null })
            .where(eq(projects.id, input.projectId))
            .run()
        }
        return db.select().from(projects).where(eq(projects.id, input.projectId)).get()!
      }

      // Look up the group by name
      const group = db
        .select()
        .from(focusGroups)
        .where(eq(focusGroups.name, configGroupName))
        .get()

      if (group) {
        // Update the DB if the group ID differs
        if (project.focusGroupId !== group.id) {
          db.update(projects)
            .set({ focusGroupId: group.id })
            .where(eq(projects.id, input.projectId))
            .run()
        }
      } else {
        // Group doesn't exist yet — create it
        const newGroupId = randomUUID()
        const existing = db.select().from(focusGroups).orderBy(asc(focusGroups.tabOrder)).all()
        const maxOrder =
          existing.length > 0 ? Math.max(...existing.map((g) => g.tabOrder)) + 1 : 0

        db.insert(focusGroups)
          .values({
            id: newGroupId,
            name: configGroupName,
            color: null,
            tabOrder: maxOrder
          })
          .run()

        db.update(projects)
          .set({ focusGroupId: newGroupId })
          .where(eq(projects.id, input.projectId))
          .run()
      }

      return db.select().from(projects).where(eq(projects.id, input.projectId)).get()!
    })
})

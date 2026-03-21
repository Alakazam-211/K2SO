import { randomUUID } from 'crypto'
import { z } from 'zod'
import { eq, asc } from 'drizzle-orm'
import { router, publicProcedure } from './trpc'
import { db } from '../db'
import { agentPresets } from '../db/schema'
import { builtInAgentPresets } from '../../../shared/agent-catalog'

export const presetsRouter = router({
  list: publicProcedure.query(() => {
    return db.select().from(agentPresets).orderBy(asc(agentPresets.sortOrder)).all()
  }),

  create: publicProcedure
    .input(
      z.object({
        label: z.string().min(1),
        command: z.string().min(1),
        icon: z.string().optional()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()

      // Get max sortOrder to append at end
      const existing = db.select().from(agentPresets).orderBy(asc(agentPresets.sortOrder)).all()
      const maxOrder =
        existing.length > 0 ? Math.max(...existing.map((p) => p.sortOrder)) + 1 : 0

      db.insert(agentPresets)
        .values({
          id,
          label: input.label,
          command: input.command,
          icon: input.icon ?? null,
          enabled: 1,
          isBuiltIn: 0,
          sortOrder: maxOrder
        })
        .run()

      return db.select().from(agentPresets).where(eq(agentPresets.id, id)).get()!
    }),

  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        label: z.string().optional(),
        command: z.string().optional(),
        icon: z.string().optional(),
        enabled: z.number().optional(),
        sortOrder: z.number().optional()
      })
    )
    .mutation(({ input }) => {
      const { id, ...updates } = input
      const setValues: Record<string, unknown> = {}
      if (updates.label !== undefined) setValues.label = updates.label
      if (updates.command !== undefined) setValues.command = updates.command
      if (updates.icon !== undefined) setValues.icon = updates.icon
      if (updates.enabled !== undefined) setValues.enabled = updates.enabled
      if (updates.sortOrder !== undefined) setValues.sortOrder = updates.sortOrder

      if (Object.keys(setValues).length > 0) {
        db.update(agentPresets).set(setValues).where(eq(agentPresets.id, id)).run()
      }

      return db.select().from(agentPresets).where(eq(agentPresets.id, id)).get()!
    }),

  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      // Prevent deleting built-in presets
      const preset = db.select().from(agentPresets).where(eq(agentPresets.id, input.id)).get()
      if (preset?.isBuiltIn) {
        throw new Error('Cannot delete built-in presets. Disable them instead.')
      }
      db.delete(agentPresets).where(eq(agentPresets.id, input.id)).run()
      return { success: true }
    }),

  reorder: publicProcedure
    .input(z.object({ ids: z.array(z.string()) }))
    .mutation(({ input }) => {
      for (let i = 0; i < input.ids.length; i++) {
        db.update(agentPresets)
          .set({ sortOrder: i })
          .where(eq(agentPresets.id, input.ids[i]))
          .run()
      }
      return { success: true }
    }),

  resetBuiltIns: publicProcedure.mutation(() => {
    // Delete all existing built-in presets
    for (const preset of builtInAgentPresets) {
      db.delete(agentPresets).where(eq(agentPresets.id, preset.id)).run()
    }

    // Re-insert from catalog
    for (const preset of builtInAgentPresets) {
      db.insert(agentPresets)
        .values({
          id: preset.id,
          label: preset.label,
          command: preset.command,
          icon: preset.icon,
          enabled: preset.enabled,
          isBuiltIn: preset.isBuiltIn,
          sortOrder: preset.sortOrder
        })
        .run()
    }

    return db.select().from(agentPresets).orderBy(asc(agentPresets.sortOrder)).all()
  })
})

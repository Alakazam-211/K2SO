import { router, publicProcedure } from './trpc'
import { projectsRouter, workspacesRouter } from './projects'
import { terminalRouter } from './terminal'
import { presetsRouter } from './presets'
import { filesystemRouter } from './filesystem'
import { gitRouter } from './git'
import { settingsRouter } from './settings'
import { projectConfigRouter } from './project-config'
import { sectionsRouter } from './sections'
import { focusGroupsRouter } from './focus-groups'

export const appRouter = router({
  ping: publicProcedure.query(() => 'pong' as const),
  projects: projectsRouter,
  workspaces: workspacesRouter,
  sections: sectionsRouter,
  focusGroups: focusGroupsRouter,
  terminal: terminalRouter,
  presets: presetsRouter,
  fs: filesystemRouter,
  git: gitRouter,
  settings: settingsRouter,
  projectConfig: projectConfigRouter
})

export type AppRouter = typeof appRouter

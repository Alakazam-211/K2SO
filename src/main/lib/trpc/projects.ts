import { z } from 'zod'
import { eq, asc } from 'drizzle-orm'
import { basename, join, extname } from 'path'
import { existsSync, readFileSync, readdirSync, statSync } from 'fs'
import { dialog, shell, nativeImage } from 'electron'
import { randomUUID } from 'crypto'
import simpleGit from 'simple-git'
import { router, publicProcedure } from './trpc'
import { db } from '../db'
import { projects, workspaces, focusGroups } from '../db/schema'
import { createFocusWindow } from '../window-manager'
import { getAllEditors, getInstalledEditors, openInEditor, clearEditorCache } from '../editors'
import { getProjectConfig } from '../project-config'

// ── Icon detection helpers ──────────────────────────────────────────────

const ICON_BASENAMES = ['favicon', 'icon', 'logo', 'app-icon']
const ICON_EXTENSIONS = ['.svg', '.png', '.ico', '.jpg', '.jpeg', '.icns']

// Priority: SVG=0 > PNG=1 > ICO=2 > JPG/JPEG=3 > ICNS=4
function extensionPriority(ext: string): number {
  switch (ext.toLowerCase()) {
    case '.svg': return 0
    case '.png': return 1
    case '.ico': return 2
    case '.jpg': case '.jpeg': return 3
    case '.icns': return 4
    default: return 99
  }
}

function isIconFilename(name: string): boolean {
  const lower = name.toLowerCase()
  const ext = extname(lower)
  const base = lower.replace(ext, '')
  return ICON_BASENAMES.includes(base) && ICON_EXTENSIONS.includes(ext)
}

/** Recursively search directory for icon files up to maxDepth levels */
function findIconFiles(dir: string, maxDepth: number, currentDepth = 0): string[] {
  const results: string[] = []
  if (currentDepth > maxDepth) return results

  try {
    const entries = readdirSync(dir, { withFileTypes: true })
    for (const entry of entries) {
      const fullPath = join(dir, entry.name)
      if (entry.isFile() && isIconFilename(entry.name)) {
        results.push(fullPath)
      } else if (entry.isDirectory() && currentDepth < maxDepth) {
        // Skip node_modules, .git, etc.
        const skip = ['node_modules', '.git', '.next', 'dist', 'out', 'coverage', '.cache']
        if (!skip.includes(entry.name)) {
          results.push(...findIconFiles(fullPath, maxDepth, currentDepth + 1))
        }
      }
    }
  } catch {
    // Permission errors, etc.
  }

  return results
}

/** Read an icon file and return a data URL, resized to targetSize for raster */
function readIconAsDataUrl(filePath: string, targetSize = 48): string | null {
  try {
    if (filePath.endsWith('.svg')) {
      const svg = readFileSync(filePath, 'utf-8')
      return `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`
    }
    const img = nativeImage.createFromPath(filePath)
    if (!img.isEmpty()) {
      const resized = img.resize({ width: targetSize, height: targetSize })
      return resized.toDataURL()
    }
  } catch {
    // skip unreadable
  }
  return null
}

/** Check package.json for icon field */
function checkPackageJsonIcon(projectPath: string): string | null {
  try {
    const pkgPath = join(projectPath, 'package.json')
    if (!existsSync(pkgPath)) return null
    const pkg = JSON.parse(readFileSync(pkgPath, 'utf-8'))
    if (pkg.icon && typeof pkg.icon === 'string') {
      const iconPath = join(projectPath, pkg.icon)
      if (existsSync(iconPath)) return iconPath
    }
    // Check build.icon (electron-builder)
    if (pkg.build?.icon && typeof pkg.build.icon === 'string') {
      const iconPath = join(projectPath, pkg.build.icon)
      if (existsSync(iconPath)) return iconPath
    }
  } catch {
    // ignore
  }
  return null
}

/** Check manifest.json / site.webmanifest for icon references */
function checkManifestIcons(projectPath: string): string | null {
  const manifestPaths = [
    'manifest.json',
    'site.webmanifest',
    'public/manifest.json',
    'public/site.webmanifest',
    'src/manifest.json'
  ]
  for (const mp of manifestPaths) {
    try {
      const fullPath = join(projectPath, mp)
      if (!existsSync(fullPath)) continue
      const manifest = JSON.parse(readFileSync(fullPath, 'utf-8'))
      if (Array.isArray(manifest.icons) && manifest.icons.length > 0) {
        // Sort by size descending, pick largest
        const sorted = [...manifest.icons].sort((a: { sizes?: string }, b: { sizes?: string }) => {
          const sizeA = parseInt(a.sizes?.split('x')[0] ?? '0', 10)
          const sizeB = parseInt(b.sizes?.split('x')[0] ?? '0', 10)
          return sizeB - sizeA
        })
        for (const icon of sorted) {
          if (icon.src) {
            const iconPath = join(projectPath, icon.src)
            if (existsSync(iconPath)) return iconPath
          }
        }
      }
    } catch {
      // ignore
    }
  }
  return null
}

/** Main detection: returns dataUrl or null */
function detectProjectIcon(projectPath: string): string | null {
  // 1. Check package.json icon field
  const pkgIcon = checkPackageJsonIcon(projectPath)
  if (pkgIcon) {
    const dataUrl = readIconAsDataUrl(pkgIcon)
    if (dataUrl) return dataUrl
  }

  // 2. Check manifest files
  const manifestIcon = checkManifestIcons(projectPath)
  if (manifestIcon) {
    const dataUrl = readIconAsDataUrl(manifestIcon)
    if (dataUrl) return dataUrl
  }

  // 3. Static well-known paths
  const staticPaths = [
    'favicon.ico', 'favicon.png', 'favicon.svg',
    'public/favicon.ico', 'public/favicon.png', 'public/favicon.svg',
    'static/favicon.ico', 'static/favicon.png',
    'icon.png', 'icon.svg', 'icon.ico',
    'app-icon.png', 'logo.png', 'logo.svg', 'logo.ico',
    '.icon.png',
    'app/favicon.ico', 'app/favicon.png', 'app/icon.ico', 'app/icon.png', 'app/icon.svg',
    'app/public/favicon.ico', 'app/public/favicon.png',
    'src/favicon.ico', 'src/favicon.png',
    'src/assets/icon.png', 'src/assets/icon.svg', 'src/assets/logo.png', 'src/assets/logo.svg',
    'src/assets/favicon.ico', 'src/assets/favicon.png',
    'src/app/favicon.ico', 'src/app/icon.png',
    'resources/icon.png', 'resources/icon.ico',
    'build/icon.png', 'build/icon.ico', 'build/icon.icns',
    'buildResources/icon.png', 'buildResources/icon.ico',
    'assets/icon.png', 'assets/logo.png'
  ]

  // Sort static paths by extension priority
  const sortedStatic = [...staticPaths].sort((a, b) => {
    return extensionPriority(extname(a)) - extensionPriority(extname(b))
  })

  for (const iconPath of sortedStatic) {
    const fullPath = join(projectPath, iconPath)
    if (existsSync(fullPath)) {
      const dataUrl = readIconAsDataUrl(fullPath)
      if (dataUrl) return dataUrl
    }
  }

  // 4. Recursive shallow search (max 2 levels deep)
  const found = findIconFiles(projectPath, 2)
  if (found.length > 0) {
    // Sort by extension priority
    found.sort((a, b) => extensionPriority(extname(a)) - extensionPriority(extname(b)))
    const dataUrl = readIconAsDataUrl(found[0])
    if (dataUrl) return dataUrl
  }

  return null
}

export const projectsRouter = router({
  list: publicProcedure.query(() => {
    return db.select().from(projects).orderBy(asc(projects.tabOrder)).all()
  }),

  create: publicProcedure
    .input(
      z.object({
        name: z.string().min(1),
        path: z.string().min(1),
        color: z.string().optional()
      })
    )
    .mutation(({ input }) => {
      const projectId = randomUUID()
      const workspaceId = randomUUID()

      // Get max tabOrder to append at end
      const existing = db.select().from(projects).orderBy(asc(projects.tabOrder)).all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((p) => p.tabOrder)) + 1 : 0

      db.insert(projects)
        .values({
          id: projectId,
          name: input.name,
          path: input.path,
          color: input.color ?? '#3b82f6',
          tabOrder: maxOrder
        })
        .run()

      db.insert(workspaces)
        .values({
          id: workspaceId,
          projectId,
          name: 'main',
          type: 'branch',
          branch: 'main'
        })
        .run()

      return db.select().from(projects).where(eq(projects.id, projectId)).get()!
    }),

  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        name: z.string().optional(),
        color: z.string().optional(),
        tabOrder: z.number().optional(),
        worktreeMode: z.number().optional()
      })
    )
    .mutation(({ input }) => {
      const { id, ...updates } = input
      const setValues: Record<string, unknown> = {}
      if (updates.name !== undefined) setValues.name = updates.name
      if (updates.color !== undefined) setValues.color = updates.color
      if (updates.tabOrder !== undefined) setValues.tabOrder = updates.tabOrder
      if (updates.worktreeMode !== undefined) setValues.worktreeMode = updates.worktreeMode

      if (Object.keys(setValues).length > 0) {
        db.update(projects).set(setValues).where(eq(projects.id, id)).run()
      }

      return db.select().from(projects).where(eq(projects.id, id)).get()!
    }),

  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      // Cascade delete is handled by the FK constraint, but delete workspaces explicitly too
      db.delete(workspaces).where(eq(workspaces.projectId, input.id)).run()
      db.delete(projects).where(eq(projects.id, input.id)).run()
      return { success: true }
    }),

  reorder: publicProcedure
    .input(z.object({ ids: z.array(z.string()) }))
    .mutation(({ input }) => {
      for (let i = 0; i < input.ids.length; i++) {
        db.update(projects).set({ tabOrder: i }).where(eq(projects.id, input.ids[i])).run()
      }
      return { success: true }
    }),

  addFromPath: publicProcedure
    .input(z.object({ path: z.string() }))
    .mutation(async ({ input }) => {
      const name = basename(input.path)

      // Check if the path is a git repository
      const isGitRepo = existsSync(join(input.path, '.git'))
      if (!isGitRepo) {
        // Double-check with git rev-parse in case of worktree or other git layout
        try {
          await simpleGit({ baseDir: input.path }).revparse(['--show-toplevel'])
        } catch {
          return { needsGitInit: true as const, path: input.path, name }
        }
      }

      const projectId = randomUUID()
      const workspaceId = randomUUID()

      const existing = db.select().from(projects).orderBy(asc(projects.tabOrder)).all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((p) => p.tabOrder)) + 1 : 0

      db.insert(projects)
        .values({
          id: projectId,
          name,
          path: input.path,
          color: '#3b82f6',
          tabOrder: maxOrder
        })
        .run()

      db.insert(workspaces)
        .values({
          id: workspaceId,
          projectId,
          name: 'main',
          type: 'branch',
          branch: 'main'
        })
        .run()

      // Reconcile focus group from .k2so/config.json if present
      const config = getProjectConfig(input.path)
      if (config.focusGroupName) {
        let group = db
          .select()
          .from(focusGroups)
          .where(eq(focusGroups.name, config.focusGroupName))
          .get()

        if (!group) {
          // Auto-create the focus group
          const newGroupId = randomUUID()
          const existingGroups = db
            .select()
            .from(focusGroups)
            .orderBy(asc(focusGroups.tabOrder))
            .all()
          const maxOrder =
            existingGroups.length > 0
              ? Math.max(...existingGroups.map((g) => g.tabOrder)) + 1
              : 0

          db.insert(focusGroups)
            .values({
              id: newGroupId,
              name: config.focusGroupName,
              color: null,
              tabOrder: maxOrder
            })
            .run()

          group = db.select().from(focusGroups).where(eq(focusGroups.id, newGroupId)).get()!
        }

        db.update(projects)
          .set({ focusGroupId: group.id })
          .where(eq(projects.id, projectId))
          .run()
      }

      return db.select().from(projects).where(eq(projects.id, projectId)).get()!
    }),

  addWithoutGit: publicProcedure
    .input(z.object({ path: z.string() }))
    .mutation(({ input }) => {
      const name = basename(input.path)
      const projectId = randomUUID()
      const workspaceId = randomUUID()

      const existing = db.select().from(projects).orderBy(asc(projects.tabOrder)).all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((p) => p.tabOrder)) + 1 : 0

      db.insert(projects)
        .values({ id: projectId, name, path: input.path, color: '#3b82f6', tabOrder: maxOrder, worktreeMode: 0 })
        .run()

      db.insert(workspaces)
        .values({ id: workspaceId, projectId, name: 'main', type: 'branch' })
        .run()

      return db.select().from(projects).where(eq(projects.id, projectId)).get()!
    }),

  initGitAndOpen: publicProcedure
    .input(z.object({ path: z.string(), branch: z.string().optional() }))
    .mutation(async ({ input }) => {
      const name = basename(input.path)
      const g = simpleGit({ baseDir: input.path })

      try {
        const branchName = input.branch?.trim() || 'main'
        await g.init([`--initial-branch=${branchName}`])
        await g.commit('Initial commit', { '--allow-empty': null })
      } catch (err: unknown) {
        const message = err instanceof Error ? err.message : String(err)
        if (message.includes('user.email') || message.includes('user.name')) {
          throw new Error(
            'Git user not configured. Run:\n  git config --global user.name "Your Name"\n  git config --global user.email "you@example.com"'
          )
        }
        throw new Error(`Failed to initialize git: ${message}`)
      }

      const projectId = randomUUID()
      const workspaceId = randomUUID()

      const existing = db.select().from(projects).orderBy(asc(projects.tabOrder)).all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((p) => p.tabOrder)) + 1 : 0

      db.insert(projects)
        .values({
          id: projectId,
          name,
          path: input.path,
          color: '#3b82f6',
          tabOrder: maxOrder
        })
        .run()

      db.insert(workspaces)
        .values({
          id: workspaceId,
          projectId,
          name: 'main',
          type: 'branch',
          branch: 'main'
        })
        .run()

      return db.select().from(projects).where(eq(projects.id, projectId)).get()!
    }),

  pickFolder: publicProcedure.mutation(async ({ ctx }) => {
    const result = await dialog.showOpenDialog(ctx.sender, {
      properties: ['openDirectory'],
      title: 'Select Project Folder'
    })

    if (result.canceled || result.filePaths.length === 0) {
      return null
    }

    return result.filePaths[0]
  }),

  openInFinder: publicProcedure
    .input(z.object({ path: z.string() }))
    .mutation(({ input }) => {
      shell.showItemInFolder(input.path)
      return { success: true }
    }),

  openFocusWindow: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .mutation(({ input }) => {
      const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
      if (!project) {
        throw new Error('Project not found')
      }
      createFocusWindow(project.id, project.name)
      return { success: true }
    }),

  getEditors: publicProcedure.query(() => {
    return getInstalledEditors()
  }),

  getAllEditors: publicProcedure.query(() => {
    return getAllEditors()
  }),

  refreshEditors: publicProcedure.mutation(() => {
    return clearEditorCache()
  }),

  openInEditor: publicProcedure
    .input(
      z.object({
        editorId: z.string(),
        path: z.string()
      })
    )
    .mutation(({ input }) => {
      openInEditor(input.editorId, input.path)
      return { success: true }
    }),

  getIcon: publicProcedure
    .input(z.object({ path: z.string(), projectId: z.string().optional() }))
    .query(({ input }) => {
      // Check DB first if projectId provided
      if (input.projectId) {
        const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
        if (project?.iconUrl) {
          return { found: true, dataUrl: project.iconUrl }
        }
      }

      // Run filesystem detection
      const dataUrl = detectProjectIcon(input.path)
      if (dataUrl) {
        // Cache in DB if projectId provided
        if (input.projectId) {
          db.update(projects).set({ iconUrl: dataUrl }).where(eq(projects.id, input.projectId)).run()
        }
        return { found: true, dataUrl }
      }

      return { found: false, dataUrl: null }
    }),

  detectIcon: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .mutation(({ input }) => {
      const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
      if (!project) throw new Error('Project not found')

      const dataUrl = detectProjectIcon(project.path)
      if (dataUrl) {
        db.update(projects).set({ iconUrl: dataUrl }).where(eq(projects.id, input.projectId)).run()
        return { found: true, dataUrl }
      }

      return { found: false, dataUrl: null }
    }),

  uploadIcon: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .mutation(async ({ input, ctx }) => {
      const project = db.select().from(projects).where(eq(projects.id, input.projectId)).get()
      if (!project) throw new Error('Project not found')

      const result = await dialog.showOpenDialog(ctx.sender, {
        title: 'Select Icon Image',
        filters: [
          { name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'svg', 'ico', 'icns'] }
        ],
        properties: ['openFile']
      })

      if (result.canceled || result.filePaths.length === 0) {
        return { dataUrl: null }
      }

      const filePath = result.filePaths[0]
      const dataUrl = readIconAsDataUrl(filePath)
      if (dataUrl) {
        db.update(projects).set({ iconUrl: dataUrl }).where(eq(projects.id, input.projectId)).run()
        return { dataUrl }
      }

      throw new Error('Could not read the selected image')
    }),

  clearIcon: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .mutation(({ input }) => {
      db.update(projects).set({ iconUrl: null }).where(eq(projects.id, input.projectId)).run()
      return { success: true }
    })
})

export const workspacesRouter = router({
  list: publicProcedure
    .input(z.object({ projectId: z.string() }))
    .query(({ input }) => {
      return db
        .select()
        .from(workspaces)
        .where(eq(workspaces.projectId, input.projectId))
        .orderBy(asc(workspaces.tabOrder))
        .all()
    }),

  create: publicProcedure
    .input(
      z.object({
        projectId: z.string(),
        name: z.string().min(1),
        type: z.string().optional(),
        branch: z.string().optional(),
        worktreePath: z.string().optional()
      })
    )
    .mutation(({ input }) => {
      const id = randomUUID()

      const existing = db
        .select()
        .from(workspaces)
        .where(eq(workspaces.projectId, input.projectId))
        .all()
      const maxOrder = existing.length > 0 ? Math.max(...existing.map((w) => w.tabOrder)) + 1 : 0

      db.insert(workspaces)
        .values({
          id,
          projectId: input.projectId,
          name: input.name,
          type: input.type ?? 'branch',
          branch: input.branch,
          worktreePath: input.worktreePath,
          tabOrder: maxOrder
        })
        .run()

      return db.select().from(workspaces).where(eq(workspaces.id, id)).get()!
    }),

  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(({ input }) => {
      db.delete(workspaces).where(eq(workspaces.id, input.id)).run()
      return { success: true }
    })
})

import { z } from 'zod'
import { join } from 'path'
import { homedir } from 'os'
import { existsSync, mkdirSync, readFileSync, writeFileSync, renameSync } from 'fs'
import { router, publicProcedure } from './trpc'

// ── Settings file path ───────────────────────────────────────────────
const SETTINGS_DIR = join(homedir(), '.k2so')
const SETTINGS_FILE = join(SETTINGS_DIR, 'settings.json')

// ── Default settings ─────────────────────────────────────────────────
export interface AppSettings {
  terminal: {
    fontFamily: string
    fontSize: number
    cursorStyle: 'bar' | 'block' | 'underline'
    scrollback: number
  }
  keybindings: Record<string, string>
  projects: Record<
    string,
    {
      defaultEditor?: string
    }
  >
  focusGroupsEnabled: boolean
  sidebarCollapsed: boolean
  leftPanelOpen: boolean
  rightPanelOpen: boolean
}

const DEFAULT_SETTINGS: AppSettings = {
  terminal: {
    fontFamily: 'MesloLGM Nerd Font',
    fontSize: 13,
    cursorStyle: 'bar',
    scrollback: 5000
  },
  keybindings: {},
  projects: {},
  focusGroupsEnabled: false,
  sidebarCollapsed: false,
  leftPanelOpen: false,
  rightPanelOpen: false
}

// ── File I/O helpers ─────────────────────────────────────────────────

function ensureDir(): void {
  if (!existsSync(SETTINGS_DIR)) {
    mkdirSync(SETTINGS_DIR, { recursive: true })
  }
}

function readSettings(): AppSettings {
  ensureDir()
  if (!existsSync(SETTINGS_FILE)) {
    return structuredClone(DEFAULT_SETTINGS)
  }
  try {
    const raw = readFileSync(SETTINGS_FILE, 'utf-8')
    const parsed = JSON.parse(raw)
    // Deep merge with defaults so new keys are always present
    return deepMerge(structuredClone(DEFAULT_SETTINGS), parsed) as AppSettings
  } catch {
    return structuredClone(DEFAULT_SETTINGS)
  }
}

function writeSettings(settings: AppSettings): void {
  ensureDir()
  const tmp = SETTINGS_FILE + '.tmp'
  writeFileSync(tmp, JSON.stringify(settings, null, 2), 'utf-8')
  renameSync(tmp, SETTINGS_FILE)
}

function deepMerge(target: Record<string, any>, source: Record<string, any>): any {
  const result = { ...target }
  for (const key of Object.keys(source)) {
    const sv = source[key]
    const tv = target[key]
    if (
      sv !== null &&
      sv !== undefined &&
      typeof sv === 'object' &&
      !Array.isArray(sv) &&
      tv !== null &&
      tv !== undefined &&
      typeof tv === 'object' &&
      !Array.isArray(tv)
    ) {
      result[key] = deepMerge(tv, sv)
    } else if (sv !== undefined) {
      result[key] = sv
    }
  }
  return result
}

// ── tRPC router ──────────────────────────────────────────────────────

export const settingsRouter = router({
  get: publicProcedure.query(() => {
    return readSettings()
  }),

  update: publicProcedure
    .input(
      z.object({
        terminal: z
          .object({
            fontFamily: z.string().optional(),
            fontSize: z.number().min(10).max(24).optional(),
            cursorStyle: z.enum(['bar', 'block', 'underline']).optional(),
            scrollback: z.number().min(500).max(100000).optional()
          })
          .optional(),
        keybindings: z.record(z.string()).optional(),
        projects: z
          .record(
            z.object({
              defaultEditor: z.string().optional()
            })
          )
          .optional(),
        focusGroupsEnabled: z.boolean().optional(),
        sidebarCollapsed: z.boolean().optional(),
        leftPanelOpen: z.boolean().optional(),
        rightPanelOpen: z.boolean().optional()
      })
    )
    .mutation(({ input }) => {
      const current = readSettings()
      const merged = deepMerge(current as any, input as any) as AppSettings
      writeSettings(merged)
      return merged
    }),

  reset: publicProcedure.mutation(() => {
    writeSettings(structuredClone(DEFAULT_SETTINGS))
    return DEFAULT_SETTINGS
  })
})

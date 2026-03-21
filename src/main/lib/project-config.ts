import { readFileSync, writeFileSync, existsSync, mkdirSync, renameSync } from 'fs'
import { join } from 'path'
import { randomUUID } from 'crypto'

// ── Types ────────────────────────────────────────────────────────────────

export interface ProjectConfig {
  setupCommand?: string
  teardownCommand?: string
  runCommand?: string
  defaultEditor?: string
  focusGroupName?: string | null
  env?: Record<string, string>
}

// ── Defaults ─────────────────────────────────────────────────────────────

const DEFAULT_CONFIG: ProjectConfig = {
  setupCommand: undefined,
  teardownCommand: undefined,
  runCommand: undefined,
  defaultEditor: undefined,
  focusGroupName: undefined,
  env: {}
}

// ── Helpers ──────────────────────────────────────────────────────────────

function readJsonFile(filePath: string): Partial<ProjectConfig> | null {
  if (!existsSync(filePath)) return null

  try {
    const raw = readFileSync(filePath, 'utf-8')
    return JSON.parse(raw) as Partial<ProjectConfig>
  } catch {
    return null
  }
}

// ── Public API ───────────────────────────────────────────────────────────

/**
 * Read and merge project configuration with three-tier resolution:
 * local overlay → project config → defaults
 */
export function getProjectConfig(projectPath: string): ProjectConfig {
  const configDir = join(projectPath, '.k2so')
  const projectConfigPath = join(configDir, 'config.json')
  const localConfigPath = join(configDir, 'config.local.json')

  const projectConf = readJsonFile(projectConfigPath)
  const localConf = readJsonFile(localConfigPath)

  // Merge: defaults ← project config ← local overlay
  const merged: ProjectConfig = { ...DEFAULT_CONFIG }

  if (projectConf) {
    if (projectConf.setupCommand !== undefined) merged.setupCommand = projectConf.setupCommand
    if (projectConf.teardownCommand !== undefined) merged.teardownCommand = projectConf.teardownCommand
    if (projectConf.runCommand !== undefined) merged.runCommand = projectConf.runCommand
    if (projectConf.defaultEditor !== undefined) merged.defaultEditor = projectConf.defaultEditor
    if (projectConf.focusGroupName !== undefined) merged.focusGroupName = projectConf.focusGroupName
    if (projectConf.env) merged.env = { ...merged.env, ...projectConf.env }
  }

  if (localConf) {
    if (localConf.setupCommand !== undefined) merged.setupCommand = localConf.setupCommand
    if (localConf.teardownCommand !== undefined) merged.teardownCommand = localConf.teardownCommand
    if (localConf.runCommand !== undefined) merged.runCommand = localConf.runCommand
    if (localConf.defaultEditor !== undefined) merged.defaultEditor = localConf.defaultEditor
    if (localConf.focusGroupName !== undefined) merged.focusGroupName = localConf.focusGroupName
    if (localConf.env) merged.env = { ...merged.env, ...localConf.env }
  }

  return merged
}

/**
 * Quick check if a project has a run command configured.
 */
export function hasRunCommand(projectPath: string): boolean {
  const config = getProjectConfig(projectPath)
  return !!config.runCommand
}

/**
 * Set a single key-value pair in the project's `.k2so/config.json`.
 * Uses atomic writes (temp file + rename) for safety.
 * Creates the `.k2so/` directory if it doesn't exist.
 */
export function setProjectConfigValue(
  projectPath: string,
  key: keyof ProjectConfig,
  value: unknown
): void {
  const configDir = join(projectPath, '.k2so')
  const configPath = join(configDir, 'config.json')

  // Ensure .k2so/ directory exists
  if (!existsSync(configDir)) {
    mkdirSync(configDir, { recursive: true })
  }

  // Read existing config (or start with empty object)
  let existing: Record<string, unknown> = {}
  if (existsSync(configPath)) {
    try {
      const raw = readFileSync(configPath, 'utf-8')
      existing = JSON.parse(raw)
    } catch {
      // If the file is corrupt, start fresh
      existing = {}
    }
  }

  // Set or remove the key
  if (value === null || value === undefined) {
    delete existing[key]
  } else {
    existing[key] = value
  }

  // Atomic write: write to temp file, then rename
  const tmpPath = join(configDir, `config.${randomUUID()}.tmp`)
  writeFileSync(tmpPath, JSON.stringify(existing, null, 2) + '\n', 'utf-8')
  renameSync(tmpPath, configPath)
}

import { execSync, spawn } from 'child_process'

export interface EditorInfo {
  id: string
  label: string
  macApp: string
  cliCommand: string
  installed: boolean
  type: 'editor' | 'terminal'
}

interface EditorDefinition {
  id: string
  label: string
  macApp: string
  cliCommand: string
  type: 'editor' | 'terminal'
}

const EDITOR_DEFINITIONS: EditorDefinition[] = [
  { id: 'cursor', label: 'Cursor', macApp: 'Cursor', cliCommand: 'cursor', type: 'editor' },
  { id: 'vscode', label: 'VS Code', macApp: 'Visual Studio Code', cliCommand: 'code', type: 'editor' },
  { id: 'vscode-insiders', label: 'VS Code Insiders', macApp: 'Visual Studio Code - Insiders', cliCommand: 'code-insiders', type: 'editor' },
  { id: 'windsurf', label: 'Windsurf', macApp: 'Windsurf', cliCommand: 'windsurf', type: 'editor' },
  { id: 'zed', label: 'Zed', macApp: 'Zed', cliCommand: 'zed', type: 'editor' },
  { id: 'sublime', label: 'Sublime Text', macApp: 'Sublime Text', cliCommand: 'subl', type: 'editor' },
  { id: 'xcode', label: 'Xcode', macApp: 'Xcode', cliCommand: 'xcode', type: 'editor' },
  { id: 'fleet', label: 'Fleet', macApp: 'Fleet', cliCommand: 'fleet', type: 'editor' },
  { id: 'webstorm', label: 'WebStorm', macApp: 'WebStorm', cliCommand: 'webstorm', type: 'editor' },
  { id: 'intellij', label: 'IntelliJ IDEA', macApp: 'IntelliJ IDEA', cliCommand: 'idea', type: 'editor' },
  { id: 'pycharm', label: 'PyCharm', macApp: 'PyCharm', cliCommand: 'pycharm', type: 'editor' },
  { id: 'goland', label: 'GoLand', macApp: 'GoLand', cliCommand: 'goland', type: 'editor' },
  { id: 'rustrover', label: 'RustRover', macApp: 'RustRover', cliCommand: 'rustrover', type: 'editor' },
  { id: 'android-studio', label: 'Android Studio', macApp: 'Android Studio', cliCommand: 'studio', type: 'editor' },
  { id: 'iterm', label: 'iTerm', macApp: 'iTerm', cliCommand: '', type: 'terminal' },
  { id: 'warp', label: 'Warp', macApp: 'Warp', cliCommand: '', type: 'terminal' },
  { id: 'ghostty', label: 'Ghostty', macApp: 'Ghostty', cliCommand: '', type: 'terminal' }
]

// ── Detection cache ──────────────────────────────────────────────────

let cachedResults: EditorInfo[] | null = null

function macAppExists(appName: string): boolean {
  try {
    execSync(`open -Ra "${appName}"`, { stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

function cliExists(cmd: string): boolean {
  if (!cmd) return false
  try {
    execSync(`which ${cmd}`, { stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

function detectAll(): EditorInfo[] {
  return EDITOR_DEFINITIONS.map((def) => ({
    id: def.id,
    label: def.label,
    macApp: def.macApp,
    cliCommand: def.cliCommand,
    type: def.type,
    installed: macAppExists(def.macApp) || cliExists(def.cliCommand)
  }))
}

// ── Public API ───────────────────────────────────────────────────────

export function getAllEditors(): EditorInfo[] {
  if (!cachedResults) {
    cachedResults = detectAll()
  }
  return cachedResults
}

export function getInstalledEditors(): EditorInfo[] {
  return getAllEditors().filter((e) => e.installed && e.type === 'editor')
}

export function clearEditorCache(): EditorInfo[] {
  cachedResults = null
  return getAllEditors()
}

export function openInEditor(editorId: string, path: string): void {
  const all = getAllEditors()
  const editor = all.find((e) => e.id === editorId)
  if (!editor) {
    throw new Error(`Unknown editor: ${editorId}`)
  }

  if (!editor.installed) {
    throw new Error(`Editor not installed: ${editor.label}`)
  }

  // Prefer macOS `open -a` for GUI apps
  if (macAppExists(editor.macApp)) {
    const child = spawn('open', ['-a', editor.macApp, path], {
      detached: true,
      stdio: 'ignore'
    })
    child.unref()
    return
  }

  // Fallback to CLI command
  if (editor.cliCommand && cliExists(editor.cliCommand)) {
    const child = spawn(editor.cliCommand, [path], {
      detached: true,
      stdio: 'ignore'
    })
    child.unref()
    return
  }

  throw new Error(`No available launch method for: ${editor.label}`)
}

import * as pty from 'node-pty'
import { app } from 'electron'
import { execSync } from 'child_process'
import { existsSync } from 'fs'
import { homedir } from 'os'

// ── Types ────────────────────────────────────────────────────────────────

interface TerminalInstance {
  pty: pty.IPty
  cwd: string
  dataListeners: Set<(data: string) => void>
  exitListeners: Set<(exitCode: number, signal?: number) => void>
}

interface CreateTerminalOptions {
  id: string
  cwd: string
  command?: string
  args?: string[]
}

// ── Resolved shell environment cache ─────────────────────────────────────

let resolvedEnv: Record<string, string> | null = null

function getDefaultShell(): string {
  // Try SHELL env var first
  if (process.env.SHELL && existsSync(process.env.SHELL)) {
    return process.env.SHELL
  }
  // Try common shell paths
  for (const sh of ['/bin/zsh', '/bin/bash', '/bin/sh']) {
    if (existsSync(sh)) return sh
  }
  return '/bin/sh'
}

function resolveShellEnv(): Record<string, string> {
  if (resolvedEnv) return resolvedEnv

  const shell = getDefaultShell()

  try {
    // Spawn a login shell to capture the full environment
    const output = execSync(`${shell} -ilc 'env'`, {
      encoding: 'utf-8',
      timeout: 5000,
      env: { ...process.env, SHELL: shell }
    })

    const env: Record<string, string> = {}
    for (const line of output.split('\n')) {
      const idx = line.indexOf('=')
      if (idx > 0) {
        const key = line.substring(0, idx)
        const value = line.substring(idx + 1)
        env[key] = value
      }
    }

    resolvedEnv = env
    return env
  } catch {
    // Fallback to process.env if shell resolution fails
    resolvedEnv = { ...process.env } as Record<string, string>
    return resolvedEnv
  }
}

function buildTerminalEnv(): Record<string, string> {
  const base = resolveShellEnv()
  const env: Record<string, string> = {}

  for (const [key, value] of Object.entries(base)) {
    // Strip Electron/Vite internal vars
    if (
      key.startsWith('ELECTRON_') ||
      key.startsWith('VITE_') ||
      key.startsWith('__vite')
    ) {
      continue
    }
    env[key] = value
  }

  // Set K2SO-specific vars
  env.TERM_PROGRAM = 'K2SO'
  env.TERM = 'xterm-256color'
  env.COLORTERM = 'truecolor'

  return env
}

// ── Terminal Manager ─────────────────────────────────────────────────────

class TerminalManager {
  private terminals = new Map<string, TerminalInstance>()

  createTerminal(options: CreateTerminalOptions): void {
    const { id, command, args } = options
    // Expand tilde to home directory
    const cwd = options.cwd === '~' ? homedir() : options.cwd.replace(/^~\//, `${homedir()}/`)

    if (this.terminals.has(id)) {
      throw new Error(`Terminal ${id} already exists`)
    }

    const env = buildTerminalEnv()

    let shell: string
    let shellArgs: string[]

    if (command) {
      // Always wrap commands in a login shell so the user sees the error
      // if the command isn't installed, rather than failing to spawn
      shell = getDefaultShell()
      const fullCommand = [command, ...(args || [])].join(' ')
      shellArgs = ['-ilc', fullCommand]
    } else {
      shell = getDefaultShell()
      shellArgs = []
    }

    // Ensure cwd exists, fallback to home directory
    const safeCwd = existsSync(cwd) ? cwd : homedir()

    // Ensure shell binary exists
    if (!existsSync(shell)) {
      console.error(`[terminal] Shell not found: ${shell}, falling back to /bin/zsh`)
      shell = '/bin/zsh'
      shellArgs = []
    }

    // Ensure PATH is always set
    if (!env.PATH) {
      env.PATH = '/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin'
    }

    console.log(`[terminal] Spawning: shell=${shell} args=${JSON.stringify(shellArgs)} cwd=${safeCwd}`)

    let ptyProcess: pty.IPty
    try {
      ptyProcess = pty.spawn(shell, shellArgs, {
        name: 'xterm-256color',
        cols: 80,
        rows: 24,
        cwd: safeCwd,
        env
      })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      console.error(`[terminal] pty.spawn failed: ${msg}`)
      throw new Error(`Failed to spawn terminal: ${msg}`)
    }

    const instance: TerminalInstance = {
      pty: ptyProcess,
      cwd: safeCwd,
      dataListeners: new Set(),
      exitListeners: new Set()
    }

    ptyProcess.onData((data: string) => {
      for (const listener of instance.dataListeners) {
        listener(data)
      }
    })

    ptyProcess.onExit(({ exitCode, signal }) => {
      for (const listener of instance.exitListeners) {
        listener(exitCode, signal)
      }
      this.terminals.delete(id)
    })

    this.terminals.set(id, instance)
  }

  writeToTerminal(id: string, data: string): void {
    const instance = this.terminals.get(id)
    if (!instance) throw new Error(`Terminal ${id} not found`)
    instance.pty.write(data)
  }

  resizeTerminal(id: string, cols: number, rows: number): void {
    const instance = this.terminals.get(id)
    if (!instance) return
    instance.pty.resize(cols, rows)
  }

  killTerminal(id: string): void {
    const instance = this.terminals.get(id)
    if (!instance) return
    instance.pty.kill()
    this.terminals.delete(id)
  }

  onData(id: string, callback: (data: string) => void): () => void {
    const instance = this.terminals.get(id)
    if (!instance) throw new Error(`Terminal ${id} not found`)
    instance.dataListeners.add(callback)
    return () => {
      instance.dataListeners.delete(callback)
    }
  }

  onExit(id: string, callback: (exitCode: number, signal?: number) => void): () => void {
    const instance = this.terminals.get(id)
    if (!instance) throw new Error(`Terminal ${id} not found`)
    instance.exitListeners.add(callback)
    return () => {
      instance.exitListeners.delete(callback)
    }
  }

  getTerminalCountForPath(path: string): number {
    let count = 0
    for (const instance of this.terminals.values()) {
      if (instance.cwd.startsWith(path)) {
        count++
      }
    }
    return count
  }

  getActiveCount(): number {
    return this.terminals.size
  }

  killAll(): void {
    for (const [id, instance] of this.terminals) {
      instance.pty.kill()
      this.terminals.delete(id)
    }
  }
}

// ── Singleton ────────────────────────────────────────────────────────────

export const terminalManager = new TerminalManager()

// Clean up all terminals when the app quits
app.on('will-quit', () => {
  terminalManager.killAll()
})

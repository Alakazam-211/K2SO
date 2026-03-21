import simpleGit, { type SimpleGit, type FileStatusResult } from 'simple-git'
import { shell } from 'electron'
import { join, dirname, basename } from 'path'
import { existsSync, mkdirSync } from 'fs'

// ── Types ────────────────────────────────────────────────────────────────────

export interface GitInfo {
  isRepo: boolean
  currentBranch: string
  ahead: number
  behind: number
  changedFiles: number
  untrackedFiles: number
}

export interface GitBranches {
  current: string
  local: string[]
  remote: string[]
}

export interface GitWorktree {
  path: string
  branch: string
  isMain: boolean
  isBare: boolean
}

export interface ChangedFile {
  path: string
  status: 'modified' | 'added' | 'deleted' | 'untracked'
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function git(cwd: string): SimpleGit {
  return simpleGit({ baseDir: cwd, binary: 'git', maxConcurrentProcesses: 4 })
}

function mapStatus(fileStatus: FileStatusResult): ChangedFile['status'] {
  if (fileStatus.index === '?' || fileStatus.working_dir === '?') return 'untracked'
  if (fileStatus.index === 'A' || fileStatus.working_dir === 'A') return 'added'
  if (fileStatus.index === 'D' || fileStatus.working_dir === 'D') return 'deleted'
  return 'modified'
}

// ── Public API ───────────────────────────────────────────────────────────────

export async function getGitInfo(projectPath: string): Promise<GitInfo> {
  try {
    const g = git(projectPath)
    const isRepo = await g.checkIsRepo()
    if (!isRepo) {
      return { isRepo: false, currentBranch: '', ahead: 0, behind: 0, changedFiles: 0, untrackedFiles: 0 }
    }

    const [status, branchSummary] = await Promise.all([g.status(), g.branch()])

    return {
      isRepo: true,
      currentBranch: status.current ?? branchSummary.current ?? '',
      ahead: status.ahead,
      behind: status.behind,
      changedFiles: status.files.filter((f) => f.index !== '?' && f.working_dir !== '?').length,
      untrackedFiles: status.files.filter((f) => f.index === '?' || f.working_dir === '?').length
    }
  } catch {
    return { isRepo: false, currentBranch: '', ahead: 0, behind: 0, changedFiles: 0, untrackedFiles: 0 }
  }
}

export async function listBranches(projectPath: string): Promise<GitBranches> {
  try {
    const g = git(projectPath)
    const summary = await g.branch(['-a'])

    const local: string[] = []
    const remote: string[] = []

    for (const [name, info] of Object.entries(summary.branches)) {
      if (name.startsWith('remotes/')) {
        // Strip "remotes/" prefix for display
        const remoteName = name.replace(/^remotes\//, '')
        // Skip HEAD pointer
        if (!remoteName.endsWith('/HEAD')) {
          remote.push(remoteName)
        }
      } else {
        local.push(info.name)
      }
    }

    return { current: summary.current, local, remote }
  } catch {
    return { current: '', local: [], remote: [] }
  }
}

export async function listWorktrees(projectPath: string): Promise<GitWorktree[]> {
  try {
    const g = git(projectPath)
    const result = await g.raw(['worktree', 'list', '--porcelain'])

    const worktrees: GitWorktree[] = []
    let current: Partial<GitWorktree> = {}

    for (const line of result.split('\n')) {
      if (line.startsWith('worktree ')) {
        current.path = line.replace('worktree ', '')
      } else if (line.startsWith('branch ')) {
        // branch refs/heads/main -> main
        current.branch = line.replace('branch refs/heads/', '')
      } else if (line === 'bare') {
        current.isBare = true
      } else if (line === '') {
        if (current.path) {
          worktrees.push({
            path: current.path,
            branch: current.branch ?? '(detached)',
            isMain: worktrees.length === 0,
            isBare: current.isBare ?? false
          })
        }
        current = {}
      }
    }

    // Handle last entry if no trailing newline
    if (current.path) {
      worktrees.push({
        path: current.path,
        branch: current.branch ?? '(detached)',
        isMain: worktrees.length === 0,
        isBare: current.isBare ?? false
      })
    }

    return worktrees
  } catch {
    return []
  }
}

export async function createWorktree(
  projectPath: string,
  branch: string,
  newBranch?: boolean
): Promise<{ path: string; branch: string }> {
  const g = git(projectPath)

  // Create worktrees directory adjacent to main repo
  const parentDir = dirname(projectPath)
  const repoName = basename(projectPath)
  const worktreesDir = join(parentDir, `${repoName}.worktrees`)

  if (!existsSync(worktreesDir)) {
    mkdirSync(worktreesDir, { recursive: true })
  }

  const worktreePath = join(worktreesDir, branch.replace(/\//g, '-'))

  if (newBranch) {
    await g.raw(['worktree', 'add', '-b', branch, worktreePath])
  } else {
    await g.raw(['worktree', 'add', worktreePath, branch])
  }

  return { path: worktreePath, branch }
}

export async function removeWorktree(
  projectPath: string,
  worktreePath: string,
  force?: boolean
): Promise<void> {
  try {
    const g = git(projectPath)
    const args = ['worktree', 'remove']
    if (force) args.push('--force')
    args.push(worktreePath)
    await g.raw(args)
  } catch {
    // Fallback: try to trash the directory and prune
    if (existsSync(worktreePath)) {
      await shell.trashItem(worktreePath)
    }
    // Prune stale worktree references
    const g = git(projectPath)
    await g.raw(['worktree', 'prune'])
  }
}

export async function getChangedFiles(projectPath: string): Promise<ChangedFile[]> {
  try {
    const g = git(projectPath)
    const status = await g.status()

    return status.files.map((f) => ({
      path: f.path,
      status: mapStatus(f)
    }))
  } catch {
    return []
  }
}

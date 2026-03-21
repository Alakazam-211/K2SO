import { useMemo } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useGitInfo, useGitChanges } from '@/hooks/useGit'

// ── Status helpers ───────────────────────────────────────────────────────────

const STATUS_CONFIG = {
  modified: { label: 'Modified', color: 'text-yellow-400', icon: 'M', bg: 'bg-yellow-400/10' },
  added: { label: 'Added', color: 'text-green-400', icon: 'A', bg: 'bg-green-400/10' },
  deleted: { label: 'Deleted', color: 'text-red-400', icon: 'D', bg: 'bg-red-400/10' },
  untracked: { label: 'Untracked', color: 'text-neutral-400', icon: 'U', bg: 'bg-neutral-400/10' }
} as const

type FileStatus = keyof typeof STATUS_CONFIG

// ── Component ────────────────────────────────────────────────────────────────

export default function ChangesPanel(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  // Determine the active worktree's path
  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)

  const workspacePath = activeWorkspace?.worktreePath ?? activeProject?.path

  const { data: gitInfo } = useGitInfo(workspacePath)
  const { data: changes } = useGitChanges(workspacePath)

  // Group files by status
  const grouped = useMemo(() => {
    const groups: Record<FileStatus, { path: string; status: FileStatus }[]> = {
      modified: [],
      added: [],
      deleted: [],
      untracked: []
    }

    for (const file of changes) {
      groups[file.status].push(file)
    }

    return groups
  }, [changes])

  const totalCount = changes.length

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  if (!gitInfo?.isRepo) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">Not a git repository</p>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Branch info header */}
      <div className="px-3 py-2 border-b border-[var(--color-border)]">
        <div className="flex items-center gap-2">
          <svg
            className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z"
            />
          </svg>
          <span className="text-xs font-medium text-[var(--color-text-primary)] truncate">
            {gitInfo.currentBranch}
          </span>
          {(gitInfo.ahead > 0 || gitInfo.behind > 0) && (
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
              {gitInfo.ahead > 0 && <span className="text-green-400">{'\u2191'}{gitInfo.ahead}</span>}
              {gitInfo.ahead > 0 && gitInfo.behind > 0 && ' '}
              {gitInfo.behind > 0 && <span className="text-red-400">{'\u2193'}{gitInfo.behind}</span>}
            </span>
          )}
        </div>
      </div>

      {/* Changes header */}
      <div className="px-3 py-1.5 border-b border-[var(--color-border)]">
        <span className="text-xs font-medium text-[var(--color-text-secondary)]">
          Changes ({totalCount})
        </span>
      </div>

      {/* File list */}
      <div className="flex-1 overflow-y-auto">
        {totalCount === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">Working tree clean</p>
          </div>
        ) : (
          <div className="py-1">
            {(Object.entries(grouped) as [FileStatus, typeof changes][]).map(
              ([status, files]) =>
                files.length > 0 && (
                  <div key={status} className="mb-1">
                    <div className="px-3 py-1">
                      <span
                        className={`text-[10px] font-semibold uppercase tracking-wider ${STATUS_CONFIG[status].color}`}
                      >
                        {STATUS_CONFIG[status].label} ({files.length})
                      </span>
                    </div>
                    {files.map((file) => (
                      <div
                        key={file.path}
                        className="flex items-center gap-2 px-3 py-0.5 hover:bg-white/[0.04] group"
                      >
                        <span
                          className={`w-4 h-4 flex items-center justify-center text-[10px] font-bold${STATUS_CONFIG[file.status].color} ${STATUS_CONFIG[file.status].bg} flex-shrink-0`}
                        >
                          {STATUS_CONFIG[file.status].icon}
                        </span>
                        <span className="text-xs text-[var(--color-text-secondary)] truncate">
                          {file.path}
                        </span>
                      </div>
                    ))}
                  </div>
                )
            )}
          </div>
        )}
      </div>
    </div>
  )
}

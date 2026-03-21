import { useState, useCallback } from 'react'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useGitInfo } from '@/hooks/useGit'
import WorktreeDialog from '@/components/Sidebar/WorktreeDialog'

// ── Git status badge (changed files count) ────────────────────────────────

function GitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span className="ml-1 text-[10px] tabular-nums font-medium px-1 bg-yellow-400/10 text-yellow-400 flex-shrink-0 leading-none">
      {count}
    </span>
  )
}

// ── Branch icon SVG ───────────────────────────────────────────────────────

function BranchIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3 flex-shrink-0 opacity-60"
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
  )
}

// ── WorktreeBar ───────────────────────────────────────────────────────────

interface WorktreeBarProps {
  project: ProjectWithWorkspaces
}

export default function WorktreeBar({ project }: WorktreeBarProps): React.JSX.Element {
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const [worktreeDialogOpen, setWorktreeDialogOpen] = useState(false)

  const handleWorkspaceClick = useCallback(
    (workspaceId: string) => {
      setActiveWorkspace(project.id, workspaceId)
    },
    [project.id, setActiveWorkspace]
  )

  const worktreeMode = project.worktreeMode === 1

  // Single worktree mode: just show the branch name
  if (!worktreeMode && project.workspaces.length <= 1) {
    const workspace = project.workspaces[0]
    const workspacePath = workspace?.worktreePath ?? project.path

    return (
      <div
        className="flex items-center h-[32px] min-h-[32px] px-3 bg-[var(--color-bg)] border-b border-[var(--color-border)] select-none no-drag"
      >
        <div className="flex items-center gap-1.5 text-xs text-[var(--color-text-muted)]">
          <BranchIcon />
          <span className="font-mono">{workspace?.branch ?? 'main'}</span>
          {workspace && <GitBadge path={workspacePath} />}
        </div>
      </div>
    )
  }

  // Group worktrees by section for the horizontal bar
  const sections = project.sections || []
  const ungroupedWorkspaces = project.workspaces.filter((ws) => !ws.sectionId)
  const hasSections = sections.length > 0

  const renderWorkspaceTab = (workspace: typeof project.workspaces[number]) => {
    const isActive = workspace.id === activeWorkspaceId
    const workspacePath = workspace.worktreePath ?? project.path

    return (
      <button
        key={workspace.id}
        className={`flex items-center gap-1.5 h-full px-3 text-xs font-mono whitespace-nowrap transition-colors border-r border-[var(--color-border)] ${
          isActive
            ? 'bg-[var(--color-accent)]/10 text-[var(--color-accent)] border-b-2 border-b-[var(--color-accent)]'
            : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.03]'
        }`}
        onClick={() => handleWorkspaceClick(workspace.id)}
      >
        <BranchIcon />
        <span>{workspace.name}</span>
        {workspace.branch && workspace.branch !== workspace.name && (
          <span className="text-[10px] opacity-50">{workspace.branch}</span>
        )}
        <GitBadge path={workspacePath} />
      </button>
    )
  }

  return (
    <>
      <div
        className="flex items-center h-[32px] min-h-[32px] bg-[var(--color-bg)] border-b border-[var(--color-border)] select-none no-drag overflow-x-auto overflow-y-hidden"
      >
        <div className="flex items-center h-full">
          {/* Ungrouped worktrees */}
          {ungroupedWorkspaces.map(renderWorkspaceTab)}

          {/* Section groups */}
          {hasSections && sections.map((section) => {
            const sectionWorkspaces = project.workspaces.filter(
              (ws) => ws.sectionId === section.id
            )

            if (sectionWorkspaces.length === 0) return null

            return (
              <div key={section.id} className="flex items-center h-full">
                {/* Section label divider */}
                <div
                  className="flex items-center h-full px-2 text-[9px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] border-r border-[var(--color-border)] select-none"
                  style={{
                    borderLeft: section.color
                      ? `3px solid ${section.color}`
                      : '1px solid var(--color-border)',
                    background: 'rgba(255,255,255,0.02)'
                  }}
                >
                  {section.name}
                </div>
                {sectionWorkspaces.map(renderWorkspaceTab)}
              </div>
            )
          })}

          {/* Add worktree button */}
          {worktreeMode && (
            <button
              className="flex items-center justify-center h-full px-2.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.03] transition-colors"
              onClick={() => setWorktreeDialogOpen(true)}
              title="New worktree"
            >
              <svg
                className="w-3.5 h-3.5"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
            </button>
          )}
        </div>
      </div>

      {worktreeDialogOpen && (
        <WorktreeDialog
          projectId={project.id}
          projectPath={project.path}
          open={true}
          onClose={() => setWorktreeDialogOpen(false)}
        />
      )}
    </>
  )
}

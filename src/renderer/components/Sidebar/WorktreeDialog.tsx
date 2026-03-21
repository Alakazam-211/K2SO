import { useState, useEffect, useCallback } from 'react'
import { trpc } from '@/lib/trpc'
import { useProjectsStore } from '@/stores/projects'

interface WorktreeDialogProps {
  projectId: string
  projectPath: string
  open: boolean
  onClose: () => void
}

interface BranchData {
  current: string
  local: string[]
  remote: string[]
}

export default function WorktreeDialog({
  projectId,
  projectPath,
  open,
  onClose
}: WorktreeDialogProps): React.JSX.Element | null {
  const [branches, setBranches] = useState<BranchData | null>(null)
  const [loading, setLoading] = useState(false)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [isNewBranch, setIsNewBranch] = useState(false)
  const [newBranchName, setNewBranchName] = useState('')
  const [selectedBranch, setSelectedBranch] = useState<string | null>(null)

  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  // Fetch branches when dialog opens
  useEffect(() => {
    if (!open) return

    setLoading(true)
    setError(null)
    setIsNewBranch(false)
    setNewBranchName('')
    setSelectedBranch(null)

    trpc.git.branches
      .query({ path: projectPath })
      .then((data) => {
        setBranches(data)
        setLoading(false)
      })
      .catch((e) => {
        setError(e instanceof Error ? e.message : 'Failed to load branches')
        setLoading(false)
      })
  }, [open, projectPath])

  // Close on Escape
  useEffect(() => {
    if (!open) return

    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [open, onClose])

  const handleCreate = useCallback(async () => {
    const branch = isNewBranch ? newBranchName.trim() : selectedBranch
    if (!branch) return

    setCreating(true)
    setError(null)

    try {
      await trpc.git.createWorktree.mutate({
        projectPath,
        branch,
        newBranch: isNewBranch,
        projectId
      })

      await fetchProjects()
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create worktree')
    } finally {
      setCreating(false)
    }
  }, [isNewBranch, newBranchName, selectedBranch, projectPath, projectId, fetchProjects, onClose])

  if (!open) return null

  const canCreate = isNewBranch ? newBranchName.trim().length > 0 : selectedBranch !== null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" />

      {/* Dialog */}
      <div className="relative w-[400px] max-h-[500px] flex flex-col bg-[var(--color-bg-secondary)]/95 backdrop-blur-xl border border-[var(--color-border)]  shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center justify-between">
          <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
            New Worktree
          </h2>
          <button
            className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
            onClick={onClose}
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Toggle: existing branch vs new branch */}
        <div className="px-4 pt-3 flex gap-2">
          <button
            className={`flex-1 px-3 py-1.5 text-xs  transition-colors ${
              !isNewBranch
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] font-medium'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] bg-white/[0.04]'
            }`}
            onClick={() => setIsNewBranch(false)}
          >
            Existing Branch
          </button>
          <button
            className={`flex-1 px-3 py-1.5 text-xs  transition-colors ${
              isNewBranch
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] font-medium'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] bg-white/[0.04]'
            }`}
            onClick={() => setIsNewBranch(true)}
          >
            New Branch
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-4 py-3 min-h-0">
          {loading ? (
            <div className="flex items-center justify-center py-8">
              <div className="w-5 h-5 border-2 border-[var(--color-accent)] border-t-transparent  animate-spin" />
            </div>
          ) : isNewBranch ? (
            <div className="space-y-3">
              <div>
                <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                  Branch name
                </label>
                <input
                  type="text"
                  value={newBranchName}
                  onChange={(e) => setNewBranchName(e.target.value)}
                  placeholder="feature/my-branch"
                  className="w-full px-3 py-2 text-xs bg-white/[0.04] border border-[var(--color-border)]  text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]/50 focus:ring-1 focus:ring-[var(--color-accent)]/25"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && canCreate && !creating) handleCreate()
                  }}
                />
              </div>
              <div>
                <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                  Base branch
                </label>
                <div className="text-xs text-[var(--color-text-secondary)] px-3 py-2 bg-white/[0.02] border border-[var(--color-border)] ">
                  {branches?.current ?? 'HEAD'}
                </div>
              </div>
            </div>
          ) : (
            <div className="space-y-0.5">
              {branches?.local.map((branch) => (
                <button
                  key={branch}
                  className={`w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left  transition-colors ${
                    selectedBranch === branch
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
                  }`}
                  onClick={() => setSelectedBranch(branch)}
                >
                  <svg
                    className="w-3 h-3 flex-shrink-0 opacity-50"
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
                  <span className="truncate">{branch}</span>
                  {branch === branches?.current && (
                    <span className="ml-auto text-[10px] text-[var(--color-text-muted)]">current</span>
                  )}
                </button>
              ))}
              {branches?.local.length === 0 && (
                <p className="text-xs text-[var(--color-text-muted)] text-center py-4">
                  No local branches found
                </p>
              )}
            </div>
          )}
        </div>

        {/* Error */}
        {error && (
          <div className="px-4 py-2 text-xs text-red-400 bg-red-400/5 border-t border-red-400/10">
            {error}
          </div>
        )}

        {/* Footer */}
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex justify-end gap-2">
          <button
            className="px-3 py-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
            onClick={onClose}
            disabled={creating}
          >
            Cancel
          </button>
          <button
            className={`px-4 py-1.5 text-xs font-medium  transition-colors ${
              canCreate && !creating
                ? 'bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90'
                : 'bg-white/[0.06] text-[var(--color-text-muted)] cursor-not-allowed'
            }`}
            onClick={handleCreate}
            disabled={!canCreate || creating}
          >
            {creating ? (
              <span className="flex items-center gap-2">
                <div className="w-3 h-3 border-2 border-white/40 border-t-white  animate-spin" />
                Creating...
              </span>
            ) : (
              'Create Worktree'
            )}
          </button>
        </div>
      </div>
    </div>
  )
}

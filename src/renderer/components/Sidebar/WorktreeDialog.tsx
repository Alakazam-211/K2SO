import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'

interface WorktreeDialogProps {
  projectId: string
  projectPath: string
  open: boolean
  onClose: () => void
}

interface BranchList {
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
  const [name, setName] = useState('')
  const [mode, setMode] = useState<'new' | 'existing'>('new')
  const [branches, setBranches] = useState<string[]>([])
  const [selectedBranch, setSelectedBranch] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  const fetchProjects = useProjectsStore((s) => s.fetchProjects)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  // Reset state and auto-focus when dialog opens
  useEffect(() => {
    if (!open) return
    setName('')
    setMode('new')
    setSelectedBranch('')
    setError(null)
    setCreating(false)
    // Focus after a tick so the input is mounted
    requestAnimationFrame(() => inputRef.current?.focus())

    // Fetch available branches
    invoke<BranchList>('git_branches', { path: projectPath })
      .then((result) => {
        setBranches(result.local.filter((b) => b !== result.current))
      })
      .catch(() => setBranches([]))
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
    const branchName = mode === 'new' ? name.trim() : selectedBranch
    if (!branchName) return

    setCreating(true)
    setError(null)

    try {
      const result = await invoke<{ workspaceId: string; path: string; branch: string }>(
        'git_create_worktree',
        {
          projectPath,
          branch: branchName,
          projectId,
          existingBranch: mode === 'existing'
        }
      )

      await fetchProjects()
      setActiveWorkspace(projectId, result.workspaceId)
      onClose()
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : 'Failed to create workspace')
      setError(msg)
    } finally {
      setCreating(false)
    }
  }, [name, selectedBranch, mode, projectPath, projectId, fetchProjects, setActiveWorkspace, onClose])

  if (!open) return null

  const canCreate = mode === 'new'
    ? name.trim().length > 0 && !creating
    : selectedBranch.length > 0 && !creating

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
      <div className="relative w-[350px] flex flex-col bg-[var(--color-bg-secondary)]/95 backdrop-blur-xl border border-[var(--color-border)] shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center justify-between">
          <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
            New Workspace
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

        {/* Mode toggle */}
        <div className="px-4 pt-3 flex gap-1">
          <button
            className={`flex-1 px-2 py-1.5 text-[11px] font-medium transition-colors ${
              mode === 'new'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] border border-[var(--color-accent)]/30'
                : 'bg-white/[0.04] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => setMode('new')}
          >
            New branch
          </button>
          <button
            className={`flex-1 px-2 py-1.5 text-[11px] font-medium transition-colors ${
              mode === 'existing'
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-accent)] border border-[var(--color-accent)]/30'
                : 'bg-white/[0.04] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => setMode('existing')}
            disabled={branches.length === 0}
            title={branches.length === 0 ? 'No other local branches available' : undefined}
          >
            Existing branch
          </button>
        </div>

        {/* Content */}
        <div className="px-4 py-4">
          {mode === 'new' ? (
            <>
              <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                Branch name
              </label>
              <input
                ref={inputRef}
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="feature/my-feature"
                className="w-full px-3 py-2 text-xs font-mono bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]/50 focus:ring-1 focus:ring-[var(--color-accent)]/25"
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && canCreate) handleCreate()
                }}
              />
              <p className="text-[11px] text-[var(--color-text-muted)] mt-2">
                Creates a new branch and workspace from the current branch.
              </p>
            </>
          ) : (
            <>
              <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">
                Select branch
              </label>
              <select
                value={selectedBranch}
                onChange={(e) => setSelectedBranch(e.target.value)}
                className="w-full px-3 py-2 text-xs font-mono bg-white/[0.04] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]/50 focus:ring-1 focus:ring-[var(--color-accent)]/25"
              >
                <option value="" disabled>Choose a branch...</option>
                {branches.map((b) => (
                  <option key={b} value={b}>{b}</option>
                ))}
              </select>
              <p className="text-[11px] text-[var(--color-text-muted)] mt-2">
                Opens an existing branch in its own workspace.
              </p>
            </>
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
            className={`px-4 py-1.5 text-xs font-medium transition-colors ${
              canCreate
                ? 'bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90'
                : 'bg-white/[0.06] text-[var(--color-text-muted)] cursor-not-allowed'
            }`}
            onClick={handleCreate}
            disabled={!canCreate}
          >
            {creating ? (
              <span className="flex items-center gap-2">
                <div className="w-3 h-3 border-2 border-white/40 border-t-white animate-spin" />
                Creating...
              </span>
            ) : (
              'Create'
            )}
          </button>
        </div>
      </div>
    </div>
  )
}

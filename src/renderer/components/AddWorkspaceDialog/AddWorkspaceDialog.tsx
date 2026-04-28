import React, { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useAddWorkspaceDialogStore, type WorkspacePreviewEntry } from '../../stores/add-workspace-dialog'

const ACTION_LABELS: Record<WorkspacePreviewEntry['action'], { label: string; className: string }> = {
  archive_and_import: {
    label: 'Archive + Import',
    className: 'text-amber-300 bg-amber-500/10 border-amber-500/30',
  },
  refresh: {
    label: 'Refresh',
    className: 'text-sky-300 bg-sky-500/10 border-sky-500/30',
  },
  create: {
    label: 'Create',
    className: 'text-emerald-300 bg-emerald-500/10 border-emerald-500/30',
  },
  marker_injected: {
    label: 'Marker block',
    className: 'text-purple-300 bg-purple-500/10 border-purple-500/30',
  },
}

function formatBytes(n: number | null): string {
  if (n == null) return ''
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

export default function AddWorkspaceDialog(): React.JSX.Element | null {
  const isOpen = useAddWorkspaceDialogStore((s) => s.isOpen)
  const isPending = useAddWorkspaceDialogStore((s) => s.isPending)
  const path = useAddWorkspaceDialogStore((s) => s.path)
  const preview = useAddWorkspaceDialogStore((s) => s.preview)
  const error = useAddWorkspaceDialogStore((s) => s.error)
  const onConfirm = useAddWorkspaceDialogStore((s) => s.onConfirm)
  const close = useAddWorkspaceDialogStore((s) => s.close)
  const setIsPending = useAddWorkspaceDialogStore((s) => s.setIsPending)
  const setError = useAddWorkspaceDialogStore((s) => s.setError)

  const [showWhy, setShowWhy] = useState<boolean>(false)
  // Three-option onboarding stage. 'choose' shows the radio picker
  // (Adopt / Start Fresh / Do it later). 'adopt-picker' shows the
  // archive_and_import entries as a sub-list so the user can pick
  // which file to promote into PROJECT.md.
  const [stage, setStage] = useState<'choose' | 'adopt-picker'>('choose')
  const [pickedSource, setPickedSource] = useState<string | null>(null)
  type OnboardingMode = 'adopt' | 'fresh' | 'later'
  const [mode, setMode] = useState<OnboardingMode>('adopt')

  // Reset to the choose stage every time the dialog reopens
  useEffect(() => {
    if (isOpen) {
      setStage('choose')
      setPickedSource(null)
      setMode('adopt')
    }
  }, [isOpen])

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) close()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isPending, close])

  const handleStartFresh = async (): Promise<void> => {
    if (!onConfirm) return
    setIsPending(true)
    try {
      await onConfirm()
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      setIsPending(false)
    }
  }

  const handleDoItLater = async (): Promise<void> => {
    if (!onConfirm || !path) return
    setIsPending(true)
    try {
      // Drop the skip flag BEFORE the regen runs, so
      // write_skill_to_all_harnesses short-circuits the harness fanout
      // (CLAUDE.md / GEMINI.md / .cursor/rules / etc. stay untouched).
      await invoke('k2so_onboarding_skip', { projectPath: path })
      await onConfirm()
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      setIsPending(false)
    }
  }

  const handleAdoptConfirm = async (): Promise<void> => {
    if (!onConfirm || !path || !pickedSource) return
    setIsPending(true)
    try {
      // Adopt FIRST: writes the body to .k2so/PROJECT.md, archives the
      // source to .k2so/migration/, removes the source so the regen
      // pipeline doesn't double-import. Internally fires a regen
      // (idempotent — onConfirm's regen will re-run and pick up the
      // seeded PROJECT.md).
      const absoluteSource = `${path}/${pickedSource}`
      await invoke('k2so_onboarding_adopt', {
        projectPath: path,
        sourcePath: absoluteSource,
      })
      await onConfirm()
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
      setIsPending(false)
    }
  }

  if (!isOpen || !path) return null

  const nonTrivialEntries = preview.filter((e) => e.action !== 'create')
  const hasUserContent = preview.some((e) => e.action === 'archive_and_import')
  const adoptableEntries = preview.filter((e) => e.action === 'archive_and_import')

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={isPending ? undefined : close}
    >
      <div
        className="w-[620px] max-h-[85vh] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Add Workspace to K2SO
          </h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1 break-all font-mono">
            {path}
          </p>
        </div>

        {/* Why — always visible, plain-language framing */}
        <div className="px-5 pb-3">
          <div className="border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-3 text-[11px] leading-relaxed text-[var(--color-text-secondary)] space-y-2">
            <p>
              <span className="font-medium text-[var(--color-text-primary)]">Tell K2SO once, every AI tool listens.</span>
            </p>
            <p>
              Each AI coding tool reads its project notes from a different file. So if you want
              Claude, Cursor, Gemini, and the others to all understand your project, you'd
              normally write the same context into each one — and remember to update them all
              every time something changes.
            </p>
            <p>
              K2SO keeps one shared knowledge file for your workspace and points every AI tool
              at it. Write your context once; every tool you use sees the same up-to-date picture.
            </p>
          </div>
        </div>

        {/* Preview list — secondary detail. Always shown during the
            adopt-picker stage (the user is actively picking a file
            from it); collapsible into a toggle in the choose stage so
            the WHY + button choices stay the dominant content. */}
        <div className="flex-1 overflow-y-auto px-5 pb-3">
          {hasUserContent && stage === 'choose' ? (
            <div className="border border-amber-500/30 bg-amber-500/10 px-3 py-2 mb-3">
              <p className="text-[11px] text-amber-200 leading-relaxed">
                This workspace already has{' '}
                {adoptableEntries.length === 1
                  ? <>a file (<span className="font-mono">{adoptableEntries[0].path}</span>) </>
                  : <>{adoptableEntries.length} files </>}
                that other CLI LLM tools read. Pick how you want K2SO to treat them.
              </p>
            </div>
          ) : null}
          {stage === 'adopt-picker' ? (
            <div className="border border-[var(--color-accent)]/30 bg-[var(--color-accent)]/5 px-3 py-2 mb-2">
              <p className="text-[11px] text-[var(--color-text-secondary)] leading-relaxed">
                Pick the file whose body should seed{' '}
                <span className="font-mono text-[var(--color-text-primary)]">.k2so/PROJECT.md</span>.
                Every CLI LLM tool will read that body via symlinks. The other files in the
                list are still archived to <span className="font-mono">.k2so/migration/</span>.
              </p>
            </div>
          ) : null}
          {stage === 'choose' ? (
            <button
              type="button"
              onClick={() => setShowWhy((v) => !v)}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] underline underline-offset-2 mb-1.5"
            >
              {showWhy ? '▾ Hide file plan' : '▸ Show file plan (what K2SO will create / archive / refresh)'}
            </button>
          ) : (
            <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
              Files in this workspace
            </div>
          )}
          <div className={`flex flex-col gap-1 ${stage === 'choose' && !showWhy ? 'hidden' : ''}`}>
            {preview.map((entry) => {
              const meta = ACTION_LABELS[entry.action]
              const isAdoptable = entry.action === 'archive_and_import'
              const isPicked = pickedSource === entry.path
              const clickable = stage === 'adopt-picker' && isAdoptable
              const dimmed = stage === 'adopt-picker' && !isAdoptable
              return (
                <div
                  key={entry.path}
                  onClick={clickable ? () => setPickedSource(entry.path) : undefined}
                  className={`flex items-start gap-2 py-1.5 px-2 border ${
                    isPicked
                      ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                      : 'border-[var(--color-border)] bg-[var(--color-bg)]/30'
                  } ${clickable ? 'cursor-pointer hover:border-[var(--color-accent)]/60' : ''} ${
                    dimmed ? 'opacity-40' : ''
                  }`}
                >
                  <div
                    className={`px-1.5 py-0.5 text-[9px] uppercase tracking-wider border ${meta.className} whitespace-nowrap`}
                  >
                    {meta.label}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                        {entry.path}
                      </span>
                      {entry.size_bytes != null ? (
                        <span className="text-[9px] text-[var(--color-text-muted)] whitespace-nowrap">
                          {formatBytes(entry.size_bytes)}
                        </span>
                      ) : null}
                    </div>
                    <div className="text-[10px] text-[var(--color-text-muted)] leading-snug">
                      {entry.note}
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
          {nonTrivialEntries.length === 0 ? (
            <p className="text-[11px] text-[var(--color-text-muted)] italic mt-2">
              No pre-existing LLM files in this workspace — K2SO is starting fresh.
            </p>
          ) : null}
        </div>

        {/* Safety footer */}
        <div className="px-5 pb-3">
          <p className="text-[10px] text-[var(--color-text-muted)] leading-snug">
            Disconnect later with{' '}
            <span className="font-mono">k2so workspace remove &lt;path&gt; --mode restore-original</span>{' '}
            to revert every file from the archive.
          </p>
        </div>

        {/* Error */}
        {error ? (
          <div className="px-5 pb-4">
            <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
              <p className="text-[11px] text-red-400 whitespace-pre-wrap">{error}</p>
            </div>
          </div>
        ) : null}

        {/* Actions */}
        {stage === 'adopt-picker' ? (
          <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
            <button
              className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
              onClick={() => { setStage('choose'); setPickedSource(null); setError(null) }}
              disabled={isPending}
            >
              ← Back
            </button>
            <button
              className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
              onClick={handleAdoptConfirm}
              disabled={isPending || !pickedSource}
            >
              {isPending ? 'Adopting…' : pickedSource ? `Adopt ${pickedSource} as PROJECT.md` : 'Pick a file above'}
            </button>
          </div>
        ) : hasUserContent ? (
          <div className="px-5 pb-5 border-t border-[var(--color-border)] pt-3">
            <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-2">
              How should K2SO treat the existing context files?
            </div>
            <div className="flex flex-col gap-2">
              <label
                className={`flex gap-2.5 p-2.5 border cursor-pointer transition-colors ${
                  mode === 'adopt'
                    ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'border-[var(--color-border)] bg-[var(--color-bg)]/30 hover:bg-[var(--color-bg)]/50'
                } ${adoptableEntries.length === 0 ? 'opacity-40 cursor-not-allowed' : ''}`}
              >
                <input
                  type="radio"
                  name="onboarding-mode"
                  value="adopt"
                  checked={mode === 'adopt'}
                  onChange={() => setMode('adopt')}
                  disabled={isPending || adoptableEntries.length === 0}
                  className="mt-0.5"
                  style={{ accentColor: 'var(--color-accent)' }}
                />
                <div className="flex-1 min-w-0">
                  <div className="text-[11px] font-medium text-[var(--color-text-primary)]">
                    Adopt one as Project Knowledge
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-0.5">
                    Already have your project context written up in one of these files? Pick it
                    and K2SO copies it into <span className="font-mono">.k2so/PROJECT.md</span> as
                    the seed every CLI LLM tool will share. The other files get archived to{' '}
                    <span className="font-mono">.k2so/migration/</span>.
                  </div>
                </div>
              </label>

              <label
                className={`flex gap-2.5 p-2.5 border cursor-pointer transition-colors ${
                  mode === 'fresh'
                    ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'border-[var(--color-border)] bg-[var(--color-bg)]/30 hover:bg-[var(--color-bg)]/50'
                }`}
              >
                <input
                  type="radio"
                  name="onboarding-mode"
                  value="fresh"
                  checked={mode === 'fresh'}
                  onChange={() => setMode('fresh')}
                  disabled={isPending}
                  className="mt-0.5"
                  style={{ accentColor: 'var(--color-accent)' }}
                />
                <div className="flex-1 min-w-0">
                  <div className="text-[11px] font-medium text-[var(--color-text-primary)]">
                    Start fresh
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-0.5">
                    Existing files are stale, tool-specific, or you'd rather start clean? K2SO
                    archives every file to <span className="font-mono">.k2so/migration/</span>{' '}
                    and starts with empty Project Knowledge you fill in deliberately. Restorable
                    later via <span className="font-mono">k2so workspace remove --mode restore-original</span>.
                  </div>
                </div>
              </label>

              <label
                className={`flex gap-2.5 p-2.5 border cursor-pointer transition-colors ${
                  mode === 'later'
                    ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'border-[var(--color-border)] bg-[var(--color-bg)]/30 hover:bg-[var(--color-bg)]/50'
                }`}
              >
                <input
                  type="radio"
                  name="onboarding-mode"
                  value="later"
                  checked={mode === 'later'}
                  onChange={() => setMode('later')}
                  disabled={isPending}
                  className="mt-0.5"
                  style={{ accentColor: 'var(--color-accent)' }}
                />
                <div className="flex-1 min-w-0">
                  <div className="text-[11px] font-medium text-[var(--color-text-primary)]">
                    Do it later
                  </div>
                  <div className="text-[10px] text-[var(--color-text-muted)] leading-snug mt-0.5">
                    Not ready to commit? K2SO leaves every existing file untouched and works in
                    K2SO-only mode for now (heartbeats, agent launches still work). Each CLI LLM
                    keeps reading its own file independently. Reversible from settings.
                  </div>
                </div>
              </label>
            </div>

            <div className="mt-3 text-[10px] text-[var(--color-text-muted)] leading-snug">
              <span className="text-[var(--color-text-secondary)]">Nothing is destroyed.</span>{' '}
              Originals always land in <span className="font-mono">.k2so/migration/</span>, and{' '}
              <span className="font-mono">k2so workspace remove --mode restore-original</span>{' '}
              brings them back.
            </div>

            <div className="flex gap-2 justify-end mt-3">
              <button
                className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
                onClick={close}
                disabled={isPending}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
                onClick={() => {
                  if (mode === 'adopt') { setStage('adopt-picker'); setError(null) }
                  else if (mode === 'fresh') { handleStartFresh() }
                  else { handleDoItLater() }
                }}
                disabled={isPending || (mode === 'adopt' && adoptableEntries.length === 0)}
              >
                {isPending
                  ? '…'
                  : mode === 'adopt'
                    ? 'Continue → pick file'
                    : mode === 'fresh'
                      ? 'Add workspace'
                      : 'Add workspace (do it later)'}
              </button>
            </div>
          </div>
        ) : (
          <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
            <button
              className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
              onClick={close}
              disabled={isPending}
            >
              Cancel
            </button>
            <button
              className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
              onClick={handleStartFresh}
              disabled={isPending}
            >
              {isPending ? 'Adding…' : 'Add workspace'}
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

import React, { useEffect, useState } from 'react'
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

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !isPending) close()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, isPending, close])

  const handleConfirm = async (): Promise<void> => {
    if (!onConfirm) return
    setIsPending(true)
    try {
      await onConfirm()
      close()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  if (!isOpen || !path) return null

  const nonTrivialEntries = preview.filter((e) => e.action !== 'create')
  const hasUserContent = preview.some((e) => e.action === 'archive_and_import')

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

        {/* Why? */}
        <div className="px-5 pb-3">
          <button
            type="button"
            onClick={() => setShowWhy((v) => !v)}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] underline underline-offset-2"
          >
            {showWhy ? '▾ Why does K2SO do this?' : '▸ Why does K2SO do this?'}
          </button>
          {showWhy ? (
            <div className="mt-2 border border-[var(--color-border)] bg-[var(--color-bg)]/40 px-3 py-2.5 text-[11px] leading-relaxed text-[var(--color-text-secondary)] space-y-1.5">
              <p>
                Every CLI LLM you use reads a different file. <span className="font-mono text-[var(--color-text-primary)]">Claude</span> reads
                <span className="font-mono text-[var(--color-text-primary)]"> CLAUDE.md</span>,
                <span className="font-mono text-[var(--color-text-primary)]"> Gemini</span> reads
                <span className="font-mono text-[var(--color-text-primary)]"> GEMINI.md</span>,
                <span className="font-mono text-[var(--color-text-primary)]"> Aider</span> reads
                <span className="font-mono text-[var(--color-text-primary)]"> .aider.conf.yml</span>,
                <span className="font-mono text-[var(--color-text-primary)]"> Cursor</span> reads
                <span className="font-mono text-[var(--color-text-primary)]"> .cursor/rules/*.mdc</span>, and so on across 12 supported tools.
              </p>
              <p>
                Without K2SO, you'd maintain the same context in 5+ different files — and each time you
                update one, the others go stale. K2SO composes <span className="font-mono text-[var(--color-text-primary)]">one canonical SKILL.md</span> from
                your <span className="font-mono text-[var(--color-text-primary)]">PROJECT.md</span>, your agent's
                <span className="font-mono text-[var(--color-text-primary)]"> AGENT.md</span>, and K2SO-managed layers — then fans it out to every tool's
                discovery path via symlinks. Edit once, every tool sees the update.
              </p>
              <p>
                Anything you already wrote in those files is preserved. K2SO archives a copy to
                <span className="font-mono text-[var(--color-text-primary)]"> .k2so/migration/</span> and imports the body into SKILL.md's
                <span className="font-mono text-[var(--color-text-primary)]"> USER_NOTES</span> section — so your accumulated memory shows up in every tool after setup, not just the one it was written to.
              </p>
            </div>
          ) : null}
        </div>

        {/* Preview list */}
        <div className="flex-1 overflow-y-auto px-5 pb-3">
          {hasUserContent ? (
            <div className="border border-amber-500/30 bg-amber-500/10 px-3 py-2 mb-3">
              <p className="text-[11px] text-amber-200 leading-relaxed">
                <span className="font-medium">Heads up:</span> this workspace has existing content K2SO will
                archive and import. Nothing is deleted — originals land in{' '}
                <span className="font-mono">.k2so/migration/</span>.
              </p>
            </div>
          ) : null}
          <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1.5">
            Plan for this workspace
          </div>
          <div className="flex flex-col gap-1">
            {preview.map((entry) => {
              const meta = ACTION_LABELS[entry.action]
              return (
                <div
                  key={entry.path}
                  className="flex items-start gap-2 py-1.5 px-2 border border-[var(--color-border)] bg-[var(--color-bg)]/30"
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
            onClick={handleConfirm}
            disabled={isPending}
          >
            {isPending ? 'Adding…' : hasUserContent ? 'Add & archive existing files' : 'Add workspace'}
          </button>
        </div>
      </div>
    </div>
  )
}

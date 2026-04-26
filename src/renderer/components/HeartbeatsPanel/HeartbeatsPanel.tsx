import { useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  useHeartbeatSessionsStore,
  type HeartbeatEntry,
} from '@/stores/heartbeat-sessions'
import { IconHeartEKG } from '@/components/icons/IconHeartEKG'
import { HeartbeatEntryRow } from './HeartbeatEntry'

/**
 * Sidebar Heartbeats panel — workspace-scoped audit surface for
 * scheduled heartbeat chat sessions. Drawer-swappable (P3.3 wires the
 * left/right placement); for v1 lives in the right rail.
 *
 * Sections:
 *   - Live      : PTY currently running (braille spinner indicator)
 *   - Resumable : has a saved session_id, no live PTY (filled dot)
 *   - Scheduled : configured but never fired (hollow dot)
 *   - Archived  : collapsed by default, persisted per-workspace
 *
 * Hidden entirely when the active workspace has no agent enabled —
 * the panel only renders when the audit surface is meaningful.
 */
export function HeartbeatsPanel(): React.JSX.Element | null {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const project = useMemo(
    () => projects.find((p) => p.id === activeProjectId) ?? null,
    [projects, activeProjectId],
  )
  const projectPath = project?.path ?? null
  const agentMode = project?.agentMode ?? 'off'

  const active = useHeartbeatSessionsStore((s) => s.active)
  const archived = useHeartbeatSessionsStore((s) => s.archived)
  const loadedFor = useHeartbeatSessionsStore((s) => s.loadedFor)
  const lastError = useHeartbeatSessionsStore((s) => s.lastError)
  const refresh = useHeartbeatSessionsStore((s) => s.refresh)
  const clear = useHeartbeatSessionsStore((s) => s.clear)

  // Refresh on workspace switch + every 5s while mounted.
  useEffect(() => {
    if (!projectPath || agentMode === 'off') {
      clear()
      return
    }
    refresh(projectPath)
    const t = setInterval(() => refresh(projectPath), 5000)
    return () => clearInterval(t)
  }, [projectPath, agentMode, refresh, clear])

  const archivedKey = projectPath ? `heartbeats.archive-collapsed.${project?.id}` : null
  const [archivedOpen, setArchivedOpen] = useState<boolean>(() => {
    if (!archivedKey) return false
    return localStorage.getItem(archivedKey) === 'open'
  })
  // Persist + reset when workspace changes.
  useEffect(() => {
    if (!archivedKey) return
    setArchivedOpen(localStorage.getItem(archivedKey) === 'open')
  }, [archivedKey])

  if (!project || agentMode === 'off') return null

  const live = active.filter((e) => e.state === 'live')
  const resumable = active.filter((e) => e.state === 'resumable')
  const scheduled = active.filter((e) => e.state === 'scheduled')
  const showingForLoadedProject = loadedFor === projectPath

  const toggleArchived = (): void => {
    const next = !archivedOpen
    setArchivedOpen(next)
    if (archivedKey) localStorage.setItem(archivedKey, next ? 'open' : 'closed')
  }

  return (
    <div className="border-t border-[var(--color-border)] py-2">
      <div className="flex items-center gap-2 px-3 mb-2">
        <IconHeartEKG className="w-3.5 h-3.5 text-[var(--color-accent)]" />
        <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
          Heartbeats
        </span>
      </div>

      {!showingForLoadedProject ? (
        <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">Loading…</div>
      ) : lastError ? (
        // Surface the error rather than silently showing an empty state —
        // a broken Tauri command would otherwise render as "no heartbeats yet".
        <div className="px-3 py-2 text-[10px] text-red-400 leading-relaxed">
          Failed to load heartbeats: {lastError}
        </div>
      ) : active.length === 0 && archived.length === 0 ? (
        <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)] leading-relaxed">
          No heartbeats yet. Open <span className="font-semibold">Settings → Heartbeats</span> to add one.
        </div>
      ) : (
        <div className="space-y-1">
          {/* Active sections — render in display priority order so live work
              sits at the top of the panel where it's most visible. */}
          {live.length > 0 && (
            <Section title="Live" entries={live} projectPath={projectPath ?? ''} />
          )}
          {resumable.length > 0 && (
            <Section title="Resumable" entries={resumable} projectPath={projectPath ?? ''} />
          )}
          {scheduled.length > 0 && (
            <Section title="Scheduled" entries={scheduled} projectPath={projectPath ?? ''} />
          )}
          {/* Archived section — collapsed by default, stays on disk for
              auditability of retired heartbeats. */}
          {archived.length > 0 && (
            <div>
              <button
                onClick={toggleArchived}
                className="w-full flex items-center gap-1.5 px-3 py-1 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] cursor-pointer no-drag"
                title={archivedOpen ? 'Collapse Archived' : 'Expand Archived'}
              >
                <svg
                  className={`w-2 h-2 transition-transform ${archivedOpen ? 'rotate-90' : ''}`}
                  viewBox="0 0 8 8"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M2 1 L6 4 L2 7" />
                </svg>
                <span>Archived ({archived.length})</span>
              </button>
              {archivedOpen && (
                <div className="space-y-0.5">
                  {archived.map((entry) => (
                    <HeartbeatEntryRow key={entry.row.id} entry={entry} projectPath={projectPath ?? ''} />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function Section({
  title,
  entries,
  projectPath,
}: {
  title: string
  entries: HeartbeatEntry[]
  projectPath: string
}): React.JSX.Element {
  return (
    <div>
      <div className="px-3 py-1 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
        {title}
      </div>
      <div className="space-y-0.5">
        {entries.map((entry) => (
          <HeartbeatEntryRow key={entry.row.id} entry={entry} projectPath={projectPath} />
        ))}
      </div>
    </div>
  )
}

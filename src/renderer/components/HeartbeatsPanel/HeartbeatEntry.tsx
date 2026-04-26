import { type HeartbeatEntry } from '@/stores/heartbeat-sessions'
import { useTabsStore } from '@/stores/tabs'

/**
 * One row in the sidebar Heartbeats panel.
 *
 * Layout:
 *   [indicator]  <name>           <relative-fired-time>
 *                <schedule-summary>
 *
 * Indicators:
 *   - 'live'       : braille spinner (animated)
 *   - 'resumable'  : filled dot
 *   - 'scheduled'  : hollow ring
 *   - 'archived'   : muted hollow ring
 *
 * Click semantics (wired in P3.3 via openHeartbeatTab):
 *   - live      : focus existing tab (no spawn)
 *   - resumable : open new tab + spawn with --resume
 *   - scheduled : open new tab + spawn fresh
 *   - archived  : open new tab + spawn with --resume (read-back; user
 *                 can keep interacting if they want)
 */
export function HeartbeatEntryRow({
  entry,
  projectPath,
}: {
  entry: HeartbeatEntry
  projectPath: string
}): React.JSX.Element {
  const openHeartbeatTab = useTabsStore((s) => s.openHeartbeatTab)

  const handleClick = (): void => {
    if (!projectPath) {
      console.warn('[heartbeats-panel] click ignored — projectPath missing')
      return
    }
    // The store handles all four states:
    //  - live      : focus existing tab via existingTerminalId match
    //  - resumable : build_launch reads agent_heartbeats.last_session_id
    //  - scheduled : build_launch finds no resume target, spawns fresh
    //  - archived  : build_launch resumes if the saved session still exists
    openHeartbeatTab(projectPath, entry.row.name, {
      existingTerminalId: entry.liveTerminalId ?? undefined,
    }).catch((err) => {
      console.warn('[heartbeats-panel] openHeartbeatTab failed:', err)
    })
  }

  return (
    <button
      onClick={handleClick}
      className="w-full px-3 py-1 flex items-center gap-2 text-left hover:bg-white/[0.04] cursor-pointer no-drag transition-colors"
      title={`${entry.row.name} — ${entry.state}`}
    >
      <span className="flex-shrink-0">
        {indicatorFor(entry)}
      </span>
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-2">
          <span className={`text-xs font-mono truncate ${entry.state === 'archived' ? 'text-[var(--color-text-muted)] line-through' : 'text-[var(--color-text-primary)]'}`}>
            {entry.row.name}
          </span>
          {entry.row.lastFired && entry.state !== 'archived' && (
            <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
              {relativeTime(entry.row.lastFired)}
            </span>
          )}
        </div>
        <div className="text-[9px] text-[var(--color-text-muted)] truncate">
          {scheduleHint(entry)}
        </div>
      </div>
    </button>
  )
}

function indicatorFor(entry: HeartbeatEntry): React.ReactNode {
  switch (entry.state) {
    case 'live':
      return <span className="braille-spinner text-[10px] text-[var(--color-accent)]" />
    case 'resumable':
      return (
        <span
          className="block w-1.5 h-1.5 rounded-full bg-[var(--color-text-secondary)]"
          aria-label="Resumable"
        />
      )
    case 'scheduled':
      return (
        <span
          className="block w-1.5 h-1.5 rounded-full border border-[var(--color-text-muted)]"
          aria-label="Scheduled"
        />
      )
    case 'archived':
      return (
        <span
          className="block w-1.5 h-1.5 rounded-full border border-[var(--color-text-muted)]/40"
          aria-label="Archived"
        />
      )
  }
}

function scheduleHint(entry: HeartbeatEntry): string {
  if (entry.state === 'archived') {
    return entry.row.archivedAt ? `Archived ${relativeTime(entry.row.archivedAt)}` : 'Archived'
  }
  if (entry.state === 'scheduled') {
    return `${entry.row.frequency} · not yet fired`
  }
  if (entry.state === 'resumable') {
    return `${entry.row.frequency} · resume on next fire`
  }
  return entry.row.frequency
}

/**
 * Compact relative time formatter ("2m", "3h", "yesterday", "Apr 12").
 * Tight by design — the row has tiny horizontal real estate.
 */
function relativeTime(iso: string): string {
  const date = new Date(iso)
  if (isNaN(date.getTime())) return ''
  const ms = Date.now() - date.getTime()
  const sec = Math.floor(ms / 1000)
  if (sec < 60) return `${sec}s ago`
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min}m ago`
  const hrs = Math.floor(min / 60)
  if (hrs < 24) return `${hrs}h ago`
  const days = Math.floor(hrs / 24)
  if (days === 1) return 'yesterday'
  if (days < 7) return `${days}d ago`
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

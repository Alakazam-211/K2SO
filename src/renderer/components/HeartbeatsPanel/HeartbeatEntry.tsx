import { useCallback, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { type HeartbeatEntry } from '@/stores/heartbeat-sessions'
import { useTabsStore } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'

/**
 * One row in the Workspace panel's Heartbeats section.
 *
 * Layout (single line):
 *   [indicator]  <name>   <Daily 9 AM>           [Launch]
 *
 * Indicators (squares, not circles, per K2SO status convention):
 *   - 'live'       : braille spinner (animated)
 *   - 'resumable'  : filled square
 *   - 'scheduled'  : hollow square
 *   - 'archived'   : muted hollow square
 *
 * Click semantics:
 *   - Click row body → openHeartbeatTab (focus live, spawn-and-resume
 *     otherwise) — connects the user to the actual chat session.
 *   - Click `Launch` → k2so_heartbeat_force_fire (spawns a fresh fire
 *     using the heartbeat's WAKEUP.md, regardless of whether a live
 *     session exists). The agent-lock check still prevents
 *     double-spawn against an already-running session.
 */
export function HeartbeatEntryRow({
  entry,
  projectPath,
}: {
  entry: HeartbeatEntry
  projectPath: string
}): React.JSX.Element {
  const openHeartbeatTab = useTabsStore((s) => s.openHeartbeatTab)
  const [busy, setBusy] = useState(false)

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

  const handleLaunch = useCallback(async (e: React.MouseEvent) => {
    // Stop the row click from also firing — Launch is its own action
    // (force-fire) distinct from "open the chat tab".
    e.stopPropagation()
    if (busy || !projectPath) return
    setBusy(true)
    try {
      await invoke<string>('k2so_heartbeat_force_fire', {
        projectPath,
        name: entry.row.name,
      })
      useToastStore.getState().addToast(`Fired heartbeat "${entry.row.name}"`, 'success', 2500)
    } catch (err) {
      useToastStore.getState().addToast(`Launch failed: ${String(err)}`, 'error', 4000)
    } finally {
      setBusy(false)
    }
  }, [busy, projectPath, entry.row.name])

  const archivedOrDisabled = entry.state === 'archived' || !entry.row.enabled

  return (
    <button
      onClick={handleClick}
      className="w-full px-1 py-1 flex items-center gap-2 text-left hover:bg-white/[0.04] cursor-pointer no-drag transition-colors"
      title={`${entry.row.name} — ${entry.state}${entry.row.enabled ? '' : ' (disabled)'}`}
    >
      <span className="flex-shrink-0">
        {indicatorFor(entry)}
      </span>
      <span
        className={`text-[11px] font-mono truncate flex-shrink-0 ${
          entry.state === 'archived'
            ? 'text-[var(--color-text-muted)] line-through'
            : entry.row.enabled
              ? 'text-[var(--color-text-primary)]'
              : 'text-[var(--color-text-muted)]'
        }`}
      >
        {entry.row.name}
      </span>
      <span className="text-[9px] text-[var(--color-text-muted)] truncate flex-1">
        {entry.row.enabled ? describeSpec(entry.row.frequency, entry.row.specJson) : 'Disabled'}
      </span>
      <button
        onClick={handleLaunch}
        disabled={busy || archivedOrDisabled}
        title={
          entry.state === 'archived'
            ? 'Restore from archive before launching'
            : !entry.row.enabled
              ? 'Enable this heartbeat before launching'
              : 'Force-fire this heartbeat now'
        }
        className="px-2 py-0.5 text-[9px] font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed flex-shrink-0"
      >
        {busy ? '…' : 'Launch'}
      </button>
    </button>
  )
}

function indicatorFor(entry: HeartbeatEntry): React.ReactNode {
  // Squares (not circles) match the WorkspacePanel's status block
  // convention — same shape as the agent-status indicator at the top
  // of the panel keeps the visual language consistent.
  switch (entry.state) {
    case 'live':
      return <span className="braille-spinner text-[10px] text-[var(--color-accent)]" />
    case 'resumable':
      return (
        <span
          className="block w-2 h-2 bg-[var(--color-text-secondary)]"
          aria-label="Resumable"
        />
      )
    case 'scheduled':
      return (
        <span
          className="block w-2 h-2 border border-[var(--color-text-muted)]"
          aria-label="Scheduled"
        />
      )
    case 'archived':
      return (
        <span
          className="block w-2 h-2 border border-[var(--color-text-muted)]/40"
          aria-label="Archived"
        />
      )
  }
}

/**
 * Compact schedule summary derived from the heartbeat row's specJson.
 * Mirrors the formatter used in HeartbeatsSection's table so users see
 * the same "Daily 9 AM" / "Weekly Mon/Wed 7 AM" / "Every 30m" labels
 * everywhere a heartbeat surfaces.
 */
function describeSpec(frequency: string, specJson: string): string {
  let v: {
    frequency?: string
    time?: string
    days?: string[]
    days_of_month?: number[]
    months?: string[]
    every_seconds?: number
  } = {}
  try {
    v = JSON.parse(specJson)
  } catch {
    return frequency
  }
  const freq = v.frequency ?? frequency
  const at = v.time ? ` ${fmt12h(v.time)}` : ''
  if (freq === 'daily') return `Daily${at}`
  if (freq === 'weekly') {
    const days = (v.days ?? [])
      .map((d) => d.charAt(0).toUpperCase() + d.slice(1, 3))
      .join('/')
    return days ? `${days}${at}` : `Weekly${at}`
  }
  if (freq === 'monthly') {
    const days = (v.days_of_month ?? []).join(',')
    return days ? `Day ${days}${at}` : `Monthly${at}`
  }
  if (freq === 'yearly') {
    const months = (v.months ?? []).join(',')
    return months ? `${months}${at}` : `Yearly${at}`
  }
  if (freq === 'hourly') {
    const mins = Math.round((v.every_seconds ?? 3600) / 60)
    return `Every ${mins}m`
  }
  return freq
}

/** Convert "HH:MM" → "h AM/PM" (minute elided when 00 to keep the
 *  schedule line tight in the narrow Workspace panel). */
function fmt12h(time: string): string {
  const [hStr, mStr] = time.split(':')
  let h = parseInt(hStr, 10)
  if (isNaN(h)) return time
  const m = mStr ?? '00'
  const ampm = h >= 12 ? 'PM' : 'AM'
  if (h === 0) h = 12
  else if (h > 12) h -= 12
  return m === '00' ? `${h} ${ampm}` : `${h}:${m} ${ampm}`
}

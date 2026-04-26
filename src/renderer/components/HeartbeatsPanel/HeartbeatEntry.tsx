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

  const openHeartbeatTab = useTabsStore((s) => s.openHeartbeatTab)

  const handleLaunch = useCallback(async (e: React.MouseEvent) => {
    // Stop the row click from also firing — Launch is its own action.
    e.stopPropagation()
    if (busy || !projectPath) return
    setBusy(true)
    const toast = useToastStore.getState()
    try {
      // Step 1 — saved session?
      if (!entry.row.lastSessionId) {
        // No saved session yet — first fire. Goes through the
        // existing spawn path which composes WAKEUP.md as
        // --append-system-prompt and saves the new session id back
        // to agent_heartbeats.last_session_id once Claude writes
        // the JSONL.
        await invoke<string>('k2so_heartbeat_force_fire', {
          projectPath,
          name: entry.row.name,
        })
        toast.addToast(`Fired heartbeat "${entry.row.name}"`, 'success', 2500)
        return
      }

      const sessionId = entry.row.lastSessionId

      // Read WAKEUP.md body (frontmatter stripped). Same content the
      // first-fire path injects via --append-system-prompt; here we
      // send it as user input into the running session.
      let wakeupBody = ''
      try {
        const wakeupAbs = `${projectPath}/${entry.row.wakeupPath}`
        const result = await invoke<{ content: string }>('fs_read_file', { path: wakeupAbs })
        wakeupBody = stripFrontmatter(result.content).trim()
      } catch (err) {
        toast.addToast(`Failed to read WAKEUP.md: ${String(err)}`, 'error', 4000)
        return
      }
      if (!wakeupBody) {
        toast.addToast(`WAKEUP.md is empty for "${entry.row.name}"`, 'error', 4000)
        return
      }

      // Step 2 — find running tab for this session_id.
      const tabsStore = useTabsStore.getState()
      let runningTabId: string | null = null
      let runningTerminalId: string | null = null
      for (const t of tabsStore.tabs) {
        for (const [, pg] of t.paneGroups) {
          for (const item of pg.items) {
            if (item.type !== 'terminal') continue
            const td = item.data as { command?: string; args?: string[]; terminalId: string }
            if (td.command === 'claude' && td.args?.includes(sessionId)) {
              runningTabId = t.id
              runningTerminalId = td.terminalId
              break
            }
          }
          if (runningTabId) break
        }
        if (runningTabId) break
      }

      if (runningTabId && runningTerminalId) {
        const exists = await invoke<boolean>('terminal_exists', { id: runningTerminalId })
        if (exists) {
          // Direct inject — paste body, then send Enter after a
          // brief delay so the paste settles. Same two-phase write
          // pattern the awareness-bus wake injection uses.
          await invoke('terminal_write', { id: runningTerminalId, data: wakeupBody })
          setTimeout(() => {
            invoke('terminal_write', { id: runningTerminalId, data: '\r' }).catch(() => {})
          }, 150)
          tabsStore.setActiveTab(runningTabId)
          toast.addToast(`Sent wakeup to "${entry.row.name}"`, 'success', 2500)
          return
        }
      }

      // Resume + inject. Open the tab via the same flow click-on-row
      // uses, then wait for the PTY to settle and write the wakeup.
      const newTabId = await openHeartbeatTab(projectPath, entry.row.name)
      if (!newTabId) {
        toast.addToast(`Couldn't open session for "${entry.row.name}"`, 'error', 4000)
        return
      }
      // Find the terminal id for the just-opened tab.
      const opened = useTabsStore.getState().tabs.find((t) => t.id === newTabId)
      const newTermId = opened
        ? (() => {
            for (const [, pg] of opened.paneGroups) {
              const item = pg.items[0]
              if (item?.type === 'terminal') {
                return (item.data as { terminalId: string }).terminalId
              }
            }
            return null
          })()
        : null
      if (!newTermId) {
        toast.addToast(`Resumed "${entry.row.name}" but couldn't inject wakeup`, 'warning', 4000)
        return
      }
      // Claude needs a few seconds to come up + render its prompt
      // before it accepts pasted input. 3s mirrors the
      // chat_history_detect_active_session post-spawn delay used
      // elsewhere when waiting for the JSONL to land.
      setTimeout(() => {
        invoke('terminal_write', { id: newTermId, data: wakeupBody })
          .then(() => {
            setTimeout(() => {
              invoke('terminal_write', { id: newTermId, data: '\r' }).catch(() => {})
            }, 150)
          })
          .catch((err) => {
            console.warn('[heartbeat-launch] post-spawn inject failed:', err)
          })
      }, 3000)
      toast.addToast(`Resumed + queued wakeup for "${entry.row.name}"`, 'success', 2500)
    } catch (err) {
      toast.addToast(`Launch failed: ${String(err)}`, 'error', 4000)
    } finally {
      setBusy(false)
    }
  }, [busy, projectPath, entry.row.name, entry.row.lastSessionId, entry.row.wakeupPath, openHeartbeatTab])

  const archivedOrDisabled = entry.state === 'archived' || !entry.row.enabled

  // Row is a div, not a button — the Launch button nests inside, and
  // HTML5 disallows nested interactive elements (browsers eject the
  // inner <button> during parsing, which broke the row click target
  // in some renderers and explained why clicking the row name did
  // nothing while Launch still worked). div + role="button" gives us
  // a clean click surface that can host the inner Launch button.
  const handleKey = (e: React.KeyboardEvent): void => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      handleClick()
    }
  }

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleClick}
      onKeyDown={handleKey}
      className="w-full px-1 py-1 flex items-center gap-2 text-left hover:bg-white/[0.04] cursor-pointer no-drag transition-colors focus:outline-none focus:bg-white/[0.04]"
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
    </div>
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

/** Strip a leading YAML frontmatter block (if any) from a markdown
 *  body. Mirrors wake.rs::strip_frontmatter so the wakeup body the
 *  Launch button pastes into a running session is the same content
 *  scheduled fires send via --append-system-prompt. */
function stripFrontmatter(content: string): string {
  if (!content.startsWith('---')) return content.trim()
  const end = content.slice(3).indexOf('---')
  if (end < 0) return content.trim()
  return content.slice(3 + end + 3).trim()
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

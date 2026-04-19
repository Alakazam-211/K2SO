import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'
import type { SettingEntry } from '../searchManifest'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { SettingDropdown } from '../controls/SettingControls'

// ── Types mirroring the backend agent_heartbeats table ────────────────

interface HeartbeatRow {
  id: string
  projectId: string
  name: string
  frequency: string
  specJson: string
  wakeupPath: string
  enabled: boolean
  lastFired: string | null
  createdAt: number
}

// Mirrors src-tauri db::schema::HeartbeatFire (camelCase via serde rename).
interface HeartbeatFire {
  id: number
  projectId: string
  agentName: string | null
  scheduleName: string | null
  firedAt: string
  mode: string
  decision: string
  reason: string | null
  inboxPriority: string | null
  inboxCount: number | null
  durationMs: number | null
}

interface ScheduleSpec {
  frequency: 'daily' | 'weekly' | 'monthly' | 'yearly' | 'hourly'
  time?: string
  days?: string[]           // weekly: mon|tue|...
  days_of_month?: number[]  // monthly
  months?: string[]         // yearly
  every_seconds?: number    // hourly
  start?: string
  end?: string
}

const WEEK_DAYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun']
const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec']

function parseSpec(json: string): ScheduleSpec {
  try {
    const parsed = JSON.parse(json)
    return parsed
  } catch {
    return { frequency: 'daily', time: '09:00' }
  }
}

/** Convert "HH:MM" 24h → "h:MM AM/PM" 12h for display. Minute '00' is
 *  elided so 9 AM reads clean instead of "9:00 AM". */
function to12h(t: string | undefined): string {
  if (!t) return ''
  const [hh, mm] = t.split(':')
  let h = parseInt(hh, 10)
  if (isNaN(h)) return t
  const ap = h >= 12 ? 'PM' : 'AM'
  if (h === 0) h = 12
  else if (h > 12) h -= 12
  return mm === '00' ? `${h} ${ap}` : `${h}:${mm} ${ap}`
}

function describeSpec(row: HeartbeatRow): string {
  const spec = parseSpec(row.specJson)
  const at = spec.time ? ` at ${to12h(spec.time)}` : ''
  switch (spec.frequency) {
    case 'daily':
      return `Every day${at}`
    case 'weekly':
      return `${(spec.days ?? []).join(', ') || '—'}${at}`
    case 'monthly':
      return `Day(s) ${(spec.days_of_month ?? []).join(', ') || '—'}${at}`
    case 'yearly':
      return `${(spec.months ?? []).join(', ')} day(s) ${(spec.days_of_month ?? []).join(', ') || '—'}${at}`
    case 'hourly':
      return `Every ${Math.round((spec.every_seconds ?? 3600) / 60)}min ${to12h(spec.start ?? '00:00')}–${to12h(spec.end ?? '23:59')}`
    default:
      return row.frequency
  }
}

function describeLastFired(lastFired: string | null): string {
  if (!lastFired) return 'Never'
  try {
    const dt = new Date(lastFired)
    return dt.toLocaleString()
  } catch {
    return lastFired
  }
}

// ── Schedule editor modal ────────────────────────────────────────────

interface ScheduleEditorProps {
  initial?: { name: string; spec: ScheduleSpec }
  onCancel: () => void
  onSave: (name: string, spec: ScheduleSpec) => Promise<void>
  isEdit: boolean
}

function ScheduleEditor({ initial, onCancel, onSave, isEdit }: ScheduleEditorProps): React.JSX.Element {
  const [name, setName] = useState(initial?.name ?? '')
  const [spec, setSpec] = useState<ScheduleSpec>(initial?.spec ?? { frequency: 'daily', time: '07:00' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const update = (patch: Partial<ScheduleSpec>): void =>
    setSpec((s) => ({ ...s, ...patch }))

  const toggleDay = (d: string): void => {
    const curr = spec.days ?? []
    update({ days: curr.includes(d) ? curr.filter((x) => x !== d) : [...curr, d] })
  }
  const toggleMonth = (m: string): void => {
    const curr = spec.months ?? []
    update({ months: curr.includes(m) ? curr.filter((x) => x !== m) : [...curr, m] })
  }

  const nameValid = /^[a-z][a-z0-9-]*[a-z0-9]$/.test(name) && !['default', 'legacy'].includes(name)

  const handleSave = async (): Promise<void> => {
    if (!nameValid && !isEdit) {
      setError('Name must be lowercase letters, digits, and hyphens (not starting/ending with hyphen, not "default" or "legacy")')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await onSave(name, spec)
    } catch (e) {
      setError(String(e))
      setBusy(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 no-drag">
      <div className="w-[520px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] p-5 shadow-2xl">
        <div className="text-sm font-medium text-[var(--color-text-primary)] mb-4">
          {isEdit ? `Edit schedule: ${initial?.name}` : 'Add heartbeat'}
        </div>

        {!isEdit && (
          <div className="mb-3">
            <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
              Name
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value.toLowerCase())}
              placeholder="daily-brief"
              className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)]"
            />
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1">
              Lowercase, hyphens, digits. Becomes the folder name under <code>heartbeats/</code>.
            </div>
          </div>
        )}

        <div className="mb-3">
          <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
            Frequency
          </label>
          <SettingDropdown
            value={spec.frequency}
            options={[
              { value: 'daily', label: 'Daily' },
              { value: 'weekly', label: 'Weekly' },
              { value: 'monthly', label: 'Monthly' },
              { value: 'yearly', label: 'Yearly' },
              { value: 'hourly', label: 'Hourly' },
            ]}
            onChange={(v) => update({ frequency: v as ScheduleSpec['frequency'] })}
          />
        </div>

        {(spec.frequency === 'daily' || spec.frequency === 'weekly' || spec.frequency === 'monthly' || spec.frequency === 'yearly') && (
          <div className="mb-3">
            <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">Time</label>
            <input
              type="time"
              value={spec.time ?? '07:00'}
              onChange={(e) => update({ time: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            />
          </div>
        )}

        {spec.frequency === 'weekly' && (
          <div className="mb-3">
            <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">Days of week</label>
            <div className="flex gap-1 flex-wrap">
              {WEEK_DAYS.map((d) => {
                const on = (spec.days ?? []).includes(d)
                return (
                  <button
                    key={d}
                    onClick={() => toggleDay(d)}
                    className={`px-2 py-1 text-[11px] cursor-pointer no-drag ${
                      on
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {d}
                  </button>
                )
              })}
            </div>
          </div>
        )}

        {(spec.frequency === 'monthly' || spec.frequency === 'yearly') && (
          <div className="mb-3">
            <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
              Day(s) of month (comma-separated 1–31)
            </label>
            <input
              type="text"
              value={(spec.days_of_month ?? []).join(',')}
              onChange={(e) =>
                update({
                  days_of_month: e.target.value
                    .split(',')
                    .map((s) => parseInt(s.trim(), 10))
                    .filter((n) => n >= 1 && n <= 31),
                })
              }
              placeholder="1, 15"
              className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
            />
          </div>
        )}

        {spec.frequency === 'yearly' && (
          <div className="mb-3">
            <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">Months</label>
            <div className="flex gap-1 flex-wrap">
              {MONTHS.map((m) => {
                const on = (spec.months ?? []).includes(m)
                return (
                  <button
                    key={m}
                    onClick={() => toggleMonth(m)}
                    className={`px-2 py-1 text-[11px] cursor-pointer no-drag ${
                      on
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {m}
                  </button>
                )
              })}
            </div>
          </div>
        )}

        {spec.frequency === 'hourly' && (
          <>
            <div className="mb-3 flex gap-3">
              <div>
                <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
                  Every (minutes)
                </label>
                <input
                  type="number"
                  min={1}
                  value={Math.round((spec.every_seconds ?? 3600) / 60)}
                  onChange={(e) => update({ every_seconds: Math.max(60, parseInt(e.target.value, 10) * 60) })}
                  className="w-24 px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                />
              </div>
              <div>
                <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">Start</label>
                <input
                  type="time"
                  value={spec.start ?? '09:00'}
                  onChange={(e) => update({ start: e.target.value })}
                  className="px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                />
              </div>
              <div>
                <label className="block text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">End</label>
                <input
                  type="time"
                  value={spec.end ?? '17:00'}
                  onChange={(e) => update({ end: e.target.value })}
                  className="px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
                />
              </div>
            </div>
          </>
        )}

        {error && <div className="text-[11px] text-red-400 mb-3">{error}</div>}

        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            disabled={busy}
            className="px-3 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={busy || (!isEdit && !nameValid)}
            className="px-3 py-1.5 text-xs bg-[var(--color-accent)] text-white cursor-pointer no-drag disabled:opacity-50"
          >
            {busy ? 'Saving…' : isEdit ? 'Update' : 'Add'}
          </button>
        </div>
      </div>
    </div>
  )
}

// ── Wakeup editor overlay (single-file AIFileEditor + AI context) ────

interface WakeupEditorProps {
  projectPath: string
  agentName: string
  heartbeat: HeartbeatRow
  otherHeartbeats: HeartbeatRow[]
  onClose: () => void
}

function WakeupEditor({ projectPath, agentName, heartbeat, otherHeartbeats, onClose }: WakeupEditorProps): React.JSX.Element | null {
  const wakeupAbs = `${projectPath}/${heartbeat.wakeupPath}`
  const wakeupDir = wakeupAbs.slice(0, wakeupAbs.lastIndexOf('/'))

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // AI context: persona (AGENT.md) full + summaries of OTHER heartbeats
  // so the AI can catch conflicts/duplication without ballooning prompt size.
  const [agentMd, setAgentMd] = useState<string>('')
  useEffect(() => {
    invoke<{ content: string }>('fs_read_file', {
      path: `${projectPath}/.k2so/agents/${agentName}/AGENT.md`,
    })
      .then((r) => setAgentMd(r.content))
      .catch(() => setAgentMd(''))
  }, [projectPath, agentName])

  const otherSummaries = useMemo(() => {
    return otherHeartbeats
      .map((h) => {
        const sched = describeSpec(h)
        return `- \`${h.name}\` (${sched}) — wakeup at \`${h.wakeupPath}\``
      })
      .join('\n')
  }, [otherHeartbeats])

  const systemPrompt = useMemo(() => {
    const parts: string[] = [
      `You're editing the WAKEUP.md for the \`${heartbeat.name}\` heartbeat of agent \`${agentName}\`.`,
      ``,
      `This file is the operational directive that fires when this specific heartbeat wakes. Other heartbeats for this agent have their own WAKEUP.md files in sibling folders — keep this one focused on its own schedule's workflow.`,
      ``,
      `## Agent persona (${agentName}/AGENT.md)`,
      ``,
      agentMd || '(AGENT.md not available)',
      ``,
    ]
    if (otherSummaries) {
      parts.push('## Other heartbeats on this agent', '', otherSummaries, '')
      parts.push(
        'If you need details on another heartbeat, `cat` its WAKEUP.md directly. Avoid duplicating instructions across heartbeats.',
        '',
      )
    }
    parts.push(
      `## File to edit`,
      ``,
      `Path: \`${wakeupAbs}\``,
      ``,
      `Start by reading the current contents with Read, then ask the user what they want this heartbeat to do when it fires.`,
    )
    return parts.join('\n')
  }, [agentName, agentMd, heartbeat.name, otherSummaries, wakeupAbs])

  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    const isClaude = agentCommand.command === 'claude'
    if (isClaude) {
      return [
        ...baseArgs,
        '--append-system-prompt',
        systemPrompt,
        `Read ${wakeupAbs} and ask me what I want this heartbeat to do.`,
      ]
    }
    return baseArgs
  }, [agentCommand, systemPrompt, wakeupAbs])

  if (!agentCommand) return null

  // Use `fixed` (viewport-relative) instead of `absolute` (nearest
  // positioned ancestor). HeartbeatsPanel now lives inside a sticky
  // right-column aside, which acts as the containing block for
  // `absolute` children — the editor would otherwise render only in
  // that narrow column. Fixed covers the whole Settings surface
  // regardless of which column launched it.
  return (
    <div className="fixed inset-0 overflow-hidden bg-[var(--color-bg)] z-50">
      <AIFileEditor
        filePath={wakeupAbs}
        watchDir={wakeupDir}
        cwd={projectPath}
        command={agentCommand.command}
        args={terminalArgs}
        title={`Heartbeat: ${heartbeat.name}`}
        warningText={`This AI session has full system access in ${projectPath}.`}
        onClose={onClose}
        onFileChange={() => { /* preview rendered inline below */ }}
        preview={<WakeupPreview path={wakeupAbs} />}
      />
    </div>
  )
}

function WakeupPreview({ path }: { path: string }): React.JSX.Element {
  const [content, setContent] = useState<string>('(loading…)')
  useEffect(() => {
    let cancel = false
    const read = (): void => {
      invoke<{ content: string }>('fs_read_file', { path })
        .then((r) => { if (!cancel) setContent(r.content) })
        .catch(() => { if (!cancel) setContent('(file not found — will be created on first save)') })
    }
    read()
    const t = setInterval(read, 2000)
    return () => { cancel = true; clearInterval(t) }
  }, [path])
  return (
    <div className="h-full overflow-auto p-4 text-xs text-[var(--color-text-primary)]">
      <div className="text-[10px] text-[var(--color-text-muted)] mb-2 font-mono truncate">{path}</div>
      <pre className="whitespace-pre-wrap font-mono">{content}</pre>
    </div>
  )
}

// ── History panel (per-workspace fire audit) ─────────────────────────

function decisionColor(decision: string): string {
  if (decision === 'fired') return 'text-green-400'
  if (decision === 'error' || decision === 'wakeup_file_missing') return 'text-red-400'
  if (decision.startsWith('skipped_')) return 'text-[var(--color-text-muted)]'
  if (decision === 'no_work') return 'text-[var(--color-text-muted)]'
  return 'text-[var(--color-text-secondary)]'
}

function shortDecision(decision: string): string {
  if (decision.startsWith('skipped_')) return 'skipped'
  return decision
}

function fmtTime(iso: string): string {
  try {
    const d = new Date(iso)
    const h12 = d.getHours() % 12 || 12
    const ap = d.getHours() >= 12 ? 'PM' : 'AM'
    const mm = String(d.getMinutes()).padStart(2, '0')
    return `${h12}:${mm} ${ap}`
  } catch {
    return iso.slice(11, 16)
  }
}

function fmtDateHeader(iso: string): string {
  try {
    const d = new Date(iso)
    const today = new Date()
    const yest = new Date()
    yest.setDate(yest.getDate() - 1)
    const sameDay = (a: Date, b: Date): boolean =>
      a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
    if (sameDay(d, today)) return 'Today'
    if (sameDay(d, yest)) return 'Yesterday'
    return d.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })
  } catch {
    return iso.slice(0, 10)
  }
}

interface HistoryPanelProps {
  projectPath: string
  /** Emits true while the workspace has zero recorded fires. Used by the
   *  parent to collapse the right column when agentMode is 'off' AND no
   *  historical audit rows exist. */
  onEmptyChange?: (empty: boolean) => void
}

export function HistoryPanel({ projectPath, onEmptyChange }: HistoryPanelProps): React.JSX.Element {
  const [fires, setFires] = useState<HeartbeatFire[]>([])
  const [loading, setLoading] = useState(true)

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<HeartbeatFire[]>('k2so_heartbeat_fires_list', {
        projectPath,
        limit: 50,
      })
      setFires(list)
      onEmptyChange?.(list.length === 0)
    } catch (e) {
      console.error('[heartbeats-history] list failed', e)
    } finally {
      setLoading(false)
    }
  }, [projectPath, onEmptyChange])

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 15000)
    return () => clearInterval(t)
  }, [refresh])

  // Group by date header (Today / Yesterday / Mon Apr 14)
  const groups = useMemo(() => {
    const map = new Map<string, HeartbeatFire[]>()
    for (const f of fires) {
      const key = fmtDateHeader(f.firedAt)
      const arr = map.get(key) ?? []
      arr.push(f)
      map.set(key, arr)
    }
    return Array.from(map.entries())
  }, [fires])

  return (
    <div>
      <div className="flex items-center justify-between mb-2">
        <div>
          <h3 className="text-xs font-medium text-[var(--color-text-primary)]">History</h3>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Last 50 heartbeat fires. Refreshes every 15s.
          </p>
        </div>
        <button
          onClick={refresh}
          title="Refresh history"
          className="w-6 h-6 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag"
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="23 4 23 10 17 10" />
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
          </svg>
        </button>
      </div>

      <div className="border border-[var(--color-border)] max-h-[420px] overflow-y-auto">
        {loading ? (
          <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">Loading…</div>
        ) : fires.length === 0 ? (
          <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">No fires recorded yet.</div>
        ) : (
          groups.map(([label, entries]) => (
            <div key={label}>
              <div className="px-3 py-1 bg-[var(--color-bg-elevated)] text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] border-b border-[var(--color-border)] sticky top-0">
                {label}
              </div>
              {entries.map((f) => {
                const inbox = f.inboxCount != null ? `${f.inboxCount}${f.inboxPriority ? ` ${f.inboxPriority}` : ''}` : null
                return (
                  <div
                    key={f.id}
                    className="px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0 text-[10px]"
                    title={f.reason ?? ''}
                  >
                    <div className="flex items-center gap-2">
                      <span className="text-[var(--color-text-muted)] font-mono tabular-nums">{fmtTime(f.firedAt)}</span>
                      {/* Show agent and schedule_name when both are present —
                          scheduleName tells you WHICH heartbeat fired
                          (daily-brief vs end-of-day) on that agent. For
                          legacy fires without a schedule we fall back to
                          the agent alone, and for workspace-level decisions
                          neither is set. */}
                      <span className="font-mono truncate">
                        {f.agentName && f.scheduleName ? (
                          <>
                            <span className="text-[var(--color-text-primary)]">{f.agentName}</span>
                            <span className="text-[var(--color-text-muted)]"> / </span>
                            <span className="text-[var(--color-text-primary)]">{f.scheduleName}</span>
                          </>
                        ) : (
                          <span className="text-[var(--color-text-primary)]">
                            {f.scheduleName ?? f.agentName ?? '(workspace)'}
                          </span>
                        )}
                      </span>
                      <span className={`ml-auto ${decisionColor(f.decision)}`}>{shortDecision(f.decision)}</span>
                    </div>
                    {f.reason && (
                      <div className="text-[9px] text-[var(--color-text-muted)] truncate">{f.reason}</div>
                    )}
                    {(inbox || f.durationMs != null) && (
                      <div className="text-[9px] text-[var(--color-text-muted)] flex gap-2">
                        {inbox && <span>inbox: {inbox}</span>}
                        {f.durationMs != null && <span>{f.durationMs}ms</span>}
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          ))
        )}
      </div>
    </div>
  )
}

// ── Reusable panel ───────────────────────────────────────────────────
// Rendered inline per-workspace inside ProjectsSection. The wrapper
// HeartbeatsSection (below) is kept as a thin shim for the Settings
// router in case we ever want a top-level view again.

interface HeartbeatsPanelProps {
  projectPath: string
  agentName: string | null
  /** Workspace agent mode (manager / agent / custom / coordinator / pod). Used
      to derive a friendly display label for the header — e.g. "Workspace
      Manager" instead of the raw dir name (`pod-leader` / `__lead__`) or the
      internal sentinel. Optional for backwards compatibility. */
  agentMode?: string | null
}

function agentModeLabel(mode: string | null | undefined, fallbackName: string | null): string {
  switch (mode) {
    case 'manager':
    case 'coordinator':
    case 'pod':
      return 'Workspace Manager'
    case 'agent':
      return 'K2SO Agent'
    case 'custom':
      // Custom agents have a meaningful display name — show it.
      return fallbackName ?? 'Custom Agent'
    default:
      return fallbackName ?? '(no agent)'
  }
}

export function HeartbeatsPanel({ projectPath, agentName: agentNameProp, agentMode }: HeartbeatsPanelProps): React.JSX.Element {
  // Shape compat: the old section component took no props and resolved
  // project from the active-project store. The panel version takes
  // explicit props so it can be embedded anywhere and still work.
  const project = useMemo(() => ({ path: projectPath } as { path: string }), [projectPath])

  const [rows, setRows] = useState<HeartbeatRow[]>([])
  const agentName = agentNameProp
  const [showAdd, setShowAdd] = useState(false)
  const [editing, setEditing] = useState<HeartbeatRow | null>(null)
  const [wakeupEditing, setWakeupEditing] = useState<HeartbeatRow | null>(null)
  // Inline rename: id of the row currently being edited and the draft value.
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')
  const [renameError, setRenameError] = useState<string | null>(null)
  const toast = useToastStore.getState()

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<HeartbeatRow[]>('k2so_heartbeat_list', { projectPath: project.path })
      setRows(list)
    } catch (e) {
      console.error('[heartbeats] list failed', e)
    }
  }, [project.path])

  useEffect(() => {
    refresh()
  }, [refresh])

  const handleAdd = async (name: string, spec: ScheduleSpec): Promise<void> => {
    if (!project) return
    await invoke('k2so_heartbeat_add', {
      projectPath: project.path,
      name,
      frequency: spec.frequency,
      specJson: JSON.stringify(spec),
    })
    toast.addToast(`Added heartbeat "${name}"`, 'success', 3000)
    setShowAdd(false)
    await refresh()
  }

  const handleEdit = async (name: string, spec: ScheduleSpec): Promise<void> => {
    if (!project) return
    await invoke('k2so_heartbeat_edit', {
      projectPath: project.path,
      name,
      frequency: spec.frequency,
      specJson: JSON.stringify(spec),
    })
    toast.addToast(`Updated heartbeat "${name}"`, 'success', 3000)
    setEditing(null)
    await refresh()
  }

  const handleRemove = async (name: string): Promise<void> => {
    if (!project) return
    if (!confirm(`Remove heartbeat "${name}" and its folder?`)) return
    await invoke('k2so_heartbeat_remove', { projectPath: project.path, name })
    toast.addToast(`Removed heartbeat "${name}"`, 'info', 3000)
    await refresh()
  }

  const handleToggle = async (row: HeartbeatRow): Promise<void> => {
    if (!project) return
    await invoke('k2so_heartbeat_set_enabled', {
      projectPath: project.path,
      name: row.name,
      enabled: !row.enabled,
    })
    await refresh()
  }

  const startRename = (row: HeartbeatRow): void => {
    setRenamingId(row.id)
    setRenameDraft(row.name)
    setRenameError(null)
  }

  const cancelRename = (): void => {
    setRenamingId(null)
    setRenameDraft('')
    setRenameError(null)
  }

  const commitRename = async (row: HeartbeatRow): Promise<void> => {
    const newName = renameDraft.trim().toLowerCase()
    if (!newName || newName === row.name) {
      cancelRename()
      return
    }
    if (!/^[a-z][a-z0-9-]*[a-z0-9]$/.test(newName) || ['default', 'legacy'].includes(newName)) {
      setRenameError('Lowercase letters, digits, and hyphens only. Not "default" or "legacy".')
      return
    }
    try {
      await invoke('k2so_heartbeat_rename', {
        projectPath: project.path,
        oldName: row.name,
        newName,
      })
      toast.addToast(`Renamed "${row.name}" → "${newName}"`, 'success', 3000)
      cancelRename()
      await refresh()
    } catch (e) {
      setRenameError(String(e))
    }
  }

  return (
    <div data-settings-id="heartbeats">
      <div className="flex items-center justify-between mb-2">
        <div>
          <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Heartbeats</h3>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Scheduled wakeups for <span className="text-[var(--color-text-secondary)]">{agentModeLabel(agentMode, agentName)}</span>. Each fires on its own cadence with its own <span className="font-mono">WAKEUP.md</span>.
          </p>
        </div>
        <button
          onClick={() => setShowAdd(true)}
          title="Add heartbeat"
          className="w-6 h-6 flex items-center justify-center text-sm leading-none bg-[var(--color-accent)] text-white cursor-pointer no-drag"
        >
          +
        </button>
      </div>

      {rows.length === 0 ? (
        <div className="p-3 border border-dashed border-[var(--color-border)] text-[10px] text-[var(--color-text-muted)]">
          No heartbeats yet. Click <strong>+ Add heartbeat</strong>.
        </div>
      ) : (
        <div className="border border-[var(--color-border)]">
          <div className="grid grid-cols-[auto_1.2fr_2fr_auto_auto] gap-3 px-3 py-1.5 bg-[var(--color-bg-elevated)] text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] border-b border-[var(--color-border)]">
            <div>On/Off</div>
            <div>Heartbeat Name</div>
            <div>Schedule (click to edit)</div>
            <div className="text-right pr-[28px]">Wakeup</div>
            <div></div>
          </div>
          {rows.map((r) => {
            const isRenaming = renamingId === r.id
            return (
              <div
                key={r.id}
                className="grid grid-cols-[auto_1.2fr_2fr_auto_auto] gap-3 px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 text-xs items-center"
              >
                {/* Col 1 — On/Off toggle (its own column now) */}
                <button
                  onClick={() => handleToggle(r)}
                  role="switch"
                  aria-checked={r.enabled}
                  className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                    r.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                  }`}
                  title={r.enabled ? 'Enabled — click to disable' : 'Disabled — click to enable'}
                >
                  <span
                    className={`w-2.5 h-2.5 bg-white block transition-transform ${
                      r.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                    }`}
                  />
                </button>

                {/* Col 2 — name (click-to-edit) + last-fired hint */}
                <div className="flex flex-col min-w-0">
                  {isRenaming ? (
                    <input
                      autoFocus
                      type="text"
                      value={renameDraft}
                      onChange={(e) => setRenameDraft(e.target.value.toLowerCase())}
                      onBlur={() => { void commitRename(r) }}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') { e.preventDefault(); void commitRename(r) }
                        else if (e.key === 'Escape') { e.preventDefault(); cancelRename() }
                      }}
                      className="font-mono text-xs px-1 py-0.5 bg-[var(--color-bg-elevated)] border border-[var(--color-accent)] text-[var(--color-text-primary)] focus:outline-none"
                    />
                  ) : (
                    <span
                      onClick={() => startRename(r)}
                      title="Click to rename"
                      className={`font-mono truncate cursor-text no-drag hover:text-[var(--color-accent)] ${r.enabled ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)] line-through'}`}
                    >
                      {r.name}
                    </span>
                  )}
                  {isRenaming && renameError ? (
                    <span className="text-[9px] text-red-400 truncate">{renameError}</span>
                  ) : (
                    <span className="text-[9px] text-[var(--color-text-muted)] truncate">
                      Last fired: {describeLastFired(r.lastFired)}
                    </span>
                  )}
                </div>

                {/* Col 2 — schedule (click to edit) */}
                <button
                  onClick={() => setEditing(r)}
                  className="text-left text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag truncate"
                  title="Click to edit schedule"
                >
                  <span className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mr-2">{r.frequency}</span>
                  {describeSpec(r)}
                </button>

                {/* Col 3 — Configure Wakeup button */}
                <button
                  onClick={() => setWakeupEditing(r)}
                  className="px-2 py-1 text-[10px] bg-[var(--color-accent)] text-white cursor-pointer no-drag justify-self-end"
                >
                  Configure Wakeup
                </button>

                {/* Col 4 — Remove (same x as Connected Workspaces) */}
                <button
                  onClick={() => handleRemove(r.name)}
                  className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-red-400 transition-colors no-drag cursor-pointer flex-shrink-0"
                  title="Remove heartbeat"
                >
                  <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <line x1="1" y1="1" x2="7" y2="7" />
                    <line x1="7" y1="1" x2="1" y2="7" />
                  </svg>
                </button>
              </div>
            )
          })}
        </div>
      )}

      {showAdd && (
        <ScheduleEditor
          isEdit={false}
          onCancel={() => setShowAdd(false)}
          onSave={handleAdd}
        />
      )}

      {editing && (
        <ScheduleEditor
          isEdit
          initial={{ name: editing.name, spec: parseSpec(editing.specJson) }}
          onCancel={() => setEditing(null)}
          onSave={(_, spec) => handleEdit(editing.name, spec)}
        />
      )}

      {wakeupEditing && agentName && (
        <WakeupEditor
          projectPath={project.path}
          agentName={agentName}
          heartbeat={wakeupEditing}
          otherHeartbeats={rows.filter((r) => r.id !== wakeupEditing.id)}
          onClose={() => { setWakeupEditing(null); refresh() }}
        />
      )}
    </div>
  )
}

// Backwards-compat alias — the old Settings router imported
// HeartbeatsSection. We removed the top-level nav entry and now
// render the panel inline inside ProjectsSection, so this is
// effectively unused, but the export avoids breaking any stale
// imports during transition.
export function HeartbeatsSection(): React.JSX.Element {
  return (
    <div className="p-6 text-xs text-[var(--color-text-muted)]">
      Heartbeats are managed inline per-workspace in the Workspaces section.
    </div>
  )
}

// ── Search manifest ──────────────────────────────────────────────────
// Empty — heartbeats are inline in the Workspaces section, and
// PROJECTS_MANIFEST already has a 'Heartbeat Schedule' entry that
// jumps users to the right place. A separate entry would land on
// a section id that no longer exists.

export const HEARTBEATS_MANIFEST: SettingEntry[] = []

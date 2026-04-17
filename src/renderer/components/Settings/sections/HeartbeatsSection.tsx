import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import type { SettingEntry } from '../searchManifest'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'

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

function describeSpec(row: HeartbeatRow): string {
  const spec = parseSpec(row.specJson)
  switch (spec.frequency) {
    case 'daily':
      return `Every day${spec.time ? ` at ${spec.time}` : ''}`
    case 'weekly':
      return `${(spec.days ?? []).join(', ') || '—'}${spec.time ? ` at ${spec.time}` : ''}`
    case 'monthly':
      return `Day(s) ${(spec.days_of_month ?? []).join(', ') || '—'}${spec.time ? ` at ${spec.time}` : ''}`
    case 'yearly':
      return `${(spec.months ?? []).join(', ')} day(s) ${(spec.days_of_month ?? []).join(', ') || '—'}${spec.time ? ` at ${spec.time}` : ''}`
    case 'hourly':
      return `Every ${Math.round((spec.every_seconds ?? 3600) / 60)}min ${spec.start ?? '00:00'}–${spec.end ?? '23:59'}`
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
          <select
            value={spec.frequency}
            onChange={(e) => update({ frequency: e.target.value as ScheduleSpec['frequency'] })}
            className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]"
          >
            <option value="daily">Daily</option>
            <option value="weekly">Weekly</option>
            <option value="monthly">Monthly</option>
            <option value="yearly">Yearly</option>
            <option value="hourly">Hourly</option>
          </select>
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

  // AI context: persona (agent.md) full + summaries of OTHER heartbeats
  // so the AI can catch conflicts/duplication without ballooning prompt size.
  const [agentMd, setAgentMd] = useState<string>('')
  useEffect(() => {
    invoke<{ content: string }>('fs_read_file', {
      path: `${projectPath}/.k2so/agents/${agentName}/agent.md`,
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
      `You're editing the wakeup.md for the \`${heartbeat.name}\` heartbeat of agent \`${agentName}\`.`,
      ``,
      `This file is the operational directive that fires when this specific heartbeat wakes. Other heartbeats for this agent have their own wakeup.md files in sibling folders — keep this one focused on its own schedule's workflow.`,
      ``,
      `## Agent persona (${agentName}/agent.md)`,
      ``,
      agentMd || '(agent.md not available)',
      ``,
    ]
    if (otherSummaries) {
      parts.push('## Other heartbeats on this agent', '', otherSummaries, '')
      parts.push(
        'If you need details on another heartbeat, `cat` its wakeup.md directly. Avoid duplicating instructions across heartbeats.',
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

  return (
    <div className="fixed inset-0 z-40 bg-[var(--color-bg-canvas)]">
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

// ── Section component ────────────────────────────────────────────────

export function HeartbeatsSection(): React.JSX.Element {
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projects = useProjectsStore((s) => s.projects)
  const project = useMemo(() => projects.find((p) => p.id === activeProjectId), [projects, activeProjectId])

  const [rows, setRows] = useState<HeartbeatRow[]>([])
  const [agentName, setAgentName] = useState<string | null>(null)
  const [showAdd, setShowAdd] = useState(false)
  const [editing, setEditing] = useState<HeartbeatRow | null>(null)
  const [wakeupEditing, setWakeupEditing] = useState<HeartbeatRow | null>(null)
  const toast = useToastStore.getState()

  const refresh = useCallback(async () => {
    if (!project) return
    try {
      const list = await invoke<HeartbeatRow[]>('k2so_heartbeat_list', { projectPath: project.path })
      setRows(list)
    } catch (e) {
      console.error('[heartbeats] list failed', e)
    }
  }, [project])

  // Resolve the workspace's primary agent for display purposes. We only
  // need the name; the backend commands infer it from agent_mode.
  const refreshAgentName = useCallback(async () => {
    if (!project) return
    try {
      // Walk .k2so/agents for a scheduleable directory. First non-template
      // non-hidden dir wins — matches find_primary_agent's ordering.
      const entries = await invoke<{ name: string; isDirectory: boolean }[]>('fs_read_dir', {
        path: `${project.path}/.k2so/agents`,
      }).catch(() => [])
      const dirs = entries.filter((e) => e.isDirectory && !e.name.startsWith('.'))
      // Prefer a non-manager/non-k2so name as a rough custom-agent heuristic;
      // ultimate source of truth is the backend anyway.
      const candidate = dirs.find((e) => !['k2so-agent', '__lead__', 'pod-leader'].includes(e.name)) ?? dirs[0]
      setAgentName(candidate?.name ?? null)
    } catch {
      setAgentName(null)
    }
  }, [project])

  useEffect(() => {
    refresh()
    refreshAgentName()
  }, [refresh, refreshAgentName])

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

  const handleRename = async (row: HeartbeatRow): Promise<void> => {
    if (!project) return
    const newName = prompt(`Rename "${row.name}" to:`, row.name)
    if (!newName || newName === row.name) return
    try {
      await invoke('k2so_heartbeat_rename', {
        projectPath: project.path,
        oldName: row.name,
        newName,
      })
      toast.addToast(`Renamed "${row.name}" → "${newName}"`, 'success', 3000)
      await refresh()
    } catch (e) {
      toast.addToast(`Rename failed: ${e}`, 'error', 5000)
    }
  }

  if (!project) {
    return (
      <div className="p-6 text-xs text-[var(--color-text-muted)]">
        Select a workspace to view its heartbeats.
      </div>
    )
  }

  return (
    <div className="p-6" data-settings-id="heartbeats">
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Heartbeats</h2>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
            Named scheduled wakeups for <span className="font-mono">{agentName ?? '(no agent)'}</span>. Each heartbeat has its own folder under <span className="font-mono">.k2so/agents/{agentName ?? 'agent'}/heartbeats/&lt;name&gt;/</span> and fires on its own schedule.
          </p>
        </div>
        <button
          onClick={() => setShowAdd(true)}
          className="px-3 py-1.5 text-xs bg-[var(--color-accent)] text-white cursor-pointer no-drag"
        >
          + Add heartbeat
        </button>
      </div>

      {rows.length === 0 ? (
        <div className="p-4 border border-dashed border-[var(--color-border)] text-[11px] text-[var(--color-text-muted)]">
          No heartbeats configured yet. Click <strong>+ Add heartbeat</strong> to schedule one.
        </div>
      ) : (
        <div className="border border-[var(--color-border)]">
          <div className="grid grid-cols-[1.5fr_2fr_1fr_auto] gap-4 px-3 py-2 bg-[var(--color-bg-elevated)] text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] border-b border-[var(--color-border)]">
            <div>Heartbeat Name</div>
            <div>Schedule</div>
            <div>Last Fired</div>
            <div className="text-right">Actions</div>
          </div>
          {rows.map((r) => (
            <div
              key={r.id}
              className="grid grid-cols-[1.5fr_2fr_1fr_auto] gap-4 px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 text-xs items-center"
            >
              <div className="font-mono text-[var(--color-text-primary)] truncate">{r.name}</div>
              <div className="text-[var(--color-text-secondary)]">
                <span className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mr-2">
                  {r.frequency}
                </span>
                {describeSpec(r)}
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)]">{describeLastFired(r.lastFired)}</div>
              <div className="flex gap-1 justify-end">
                <button
                  onClick={() => handleToggle(r)}
                  className={`px-2 py-1 text-[10px] cursor-pointer no-drag ${
                    r.enabled ? 'bg-green-500/10 text-green-400' : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)]'
                  }`}
                  title={r.enabled ? 'Click to disable' : 'Click to enable'}
                >
                  {r.enabled ? 'on' : 'off'}
                </button>
                <button
                  onClick={() => setWakeupEditing(r)}
                  className="px-2 py-1 text-[10px] bg-[var(--color-accent)] text-white cursor-pointer no-drag"
                >
                  Configure Wakeup
                </button>
                <button
                  onClick={() => setEditing(r)}
                  className="px-2 py-1 text-[10px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] cursor-pointer no-drag"
                >
                  Edit schedule
                </button>
                <button
                  onClick={() => handleRename(r)}
                  className="px-2 py-1 text-[10px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] cursor-pointer no-drag"
                >
                  Rename
                </button>
                <button
                  onClick={() => handleRemove(r.name)}
                  className="px-2 py-1 text-[10px] bg-[var(--color-bg-elevated)] text-red-400 border border-[var(--color-border)] cursor-pointer no-drag"
                >
                  Remove
                </button>
              </div>
            </div>
          ))}
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

// ── Search manifest ──────────────────────────────────────────────────

export const HEARTBEATS_MANIFEST: SettingEntry[] = [
  {
    id: 'heartbeats-list',
    section: 'heartbeats',
    label: 'Heartbeats',
    description: 'Named scheduled wakeups for the workspace agent',
    keywords: ['heartbeat', 'schedule', 'cron', 'daily', 'weekly', 'wakeup'],
  },
]

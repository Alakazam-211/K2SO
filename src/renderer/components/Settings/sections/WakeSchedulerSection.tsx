import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'
import type { SettingEntry } from '../searchManifest'

export const WAKE_SCHEDULER_MANIFEST: SettingEntry[] = [
  {
    id: 'wake-scheduler.mode',
    section: 'wake-scheduler',
    label: 'Heartbeat Wake Mode',
    description: 'Off / on-demand / scheduled heartbeat wakes',
    keywords: ['wake', 'heartbeat', 'scheduler', 'launchd', 'sleep', 'lid'],
  },
  {
    id: 'wake-scheduler.interval',
    section: 'wake-scheduler',
    label: 'Wake Interval',
    description: 'Minutes between scheduled heartbeat fires',
    keywords: ['interval', 'minutes', 'cadence', 'frequency'],
  },
  {
    id: 'wake-scheduler.wake-system',
    section: 'wake-scheduler',
    label: 'Wake System From Sleep',
    description: 'Let launchd wake a sleeping laptop to fire heartbeats (lid-closed overnight work)',
    keywords: ['wake', 'sleep', 'lid', 'overnight', 'battery', 'wakesystem'],
  },
]

type WakeMode = 'off' | 'on_demand' | 'heartbeat'

interface WakeSchedulerSettings {
  mode: WakeMode
  intervalMinutes: number
  wakeSystem: boolean
}

interface AppSettingsShape {
  wakeScheduler?: WakeSchedulerSettings
}

const DEFAULT_SETTINGS: WakeSchedulerSettings = {
  mode: 'on_demand',
  intervalMinutes: 5,
  wakeSystem: false,
}

const MODE_DESCRIPTIONS: Record<WakeMode, string> = {
  off:
    'No launchd plist. Heartbeats only fire while K2SO is open. Agents sit idle when you quit.',
  on_demand:
    'Heartbeats fire while K2SO is open. The daemon stays running in the background after you quit, but the system will not wake itself.',
  heartbeat:
    'launchd fires scheduled heartbeats every N minutes. With "Wake System From Sleep" on, the laptop wakes from sleep (lid closed, on battery) to run agents — the configuration that makes overnight agent work possible.',
}

export function WakeSchedulerSection(): React.JSX.Element {
  const [loaded, setLoaded] = useState(false)
  const [settings, setSettings] = useState<WakeSchedulerSettings>(DEFAULT_SETTINGS)
  // Last successfully-persisted snapshot. `dirty` is computed from
  // deep-equality against this — there's no separate `setDirty` flag
  // that can drift out of sync with the actual on-disk state. Apply
  // updates this AFTER the invoke succeeds; any later edit that
  // matches the snapshot byte-for-byte clears the indicator.
  const [lastApplied, setLastApplied] = useState<WakeSchedulerSettings>(DEFAULT_SETTINGS)
  const [applying, setApplying] = useState(false)
  const toast = useToastStore((s) => s.push)

  const dirty = useMemo(
    () => JSON.stringify(settings) !== JSON.stringify(lastApplied),
    [settings, lastApplied],
  )

  // Load settings on mount.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const app = await invoke<AppSettingsShape>('settings_get')
        if (cancelled) return
        const cur = app.wakeScheduler ?? DEFAULT_SETTINGS
        const normalized: WakeSchedulerSettings = {
          mode: (cur.mode as WakeMode) ?? 'on_demand',
          intervalMinutes: cur.intervalMinutes ?? 5,
          wakeSystem: cur.wakeSystem ?? false,
        }
        setSettings(normalized)
        setLastApplied(normalized)
      } catch {
        setSettings(DEFAULT_SETTINGS)
        setLastApplied(DEFAULT_SETTINGS)
      } finally {
        if (!cancelled) setLoaded(true)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const update = useCallback((patch: Partial<WakeSchedulerSettings>) => {
    setSettings((s) => ({ ...s, ...patch }))
  }, [])

  const handleApply = useCallback(async () => {
    setApplying(true)
    try {
      await invoke('settings_update', {
        updates: {
          wakeScheduler: {
            mode: settings.mode,
            intervalMinutes: settings.intervalMinutes,
            wakeSystem: settings.wakeSystem,
          },
        },
      })
      const msg = await invoke<string>('k2so_agents_apply_wake_scheduler')
      toast(msg, 'success')
      // Snapshot the just-persisted shape. Any subsequent edit that
      // happens to bring `settings` back to this value clears the
      // dirty indicator — the previous boolean-flag implementation
      // couldn't do that.
      setLastApplied(settings)
    } catch (err) {
      toast(`Failed to apply: ${String(err)}`, 'error')
    } finally {
      setApplying(false)
    }
  }, [settings, toast])

  if (!loaded) {
    return <div className="text-[10px] text-[var(--color-text-muted)]">Loading…</div>
  }

  return (
    // Constrain to 1/3 of the settings content width so the page can
    // grow a right-side panel later (heartbeat preview, fire history,
    // etc.) without rewriting the layout. `w-1/3` is relative to the
    // parent container the Settings shell lays out for us.
    <div data-settings-id="heartbeats" className="w-1/3 min-w-[280px]">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        Heartbeats
        <span
          className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
          title="This feature is in beta — interface and behavior may change"
        >
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed">
        How launchd fires heartbeats — and whether it wakes your laptop from sleep
        to do it. Configures{' '}
        <code className="text-[var(--color-text-secondary)] font-mono">
          ~/Library/LaunchAgents/com.k2so.agent-heartbeat.plist
        </code>
        . Heartbeat schedules themselves are configured per-workspace in{' '}
        <span className="text-[var(--color-text-secondary)]">Workspaces → Heartbeats</span>.
      </p>

      {/* Mode — bottom-border only renders when the heartbeat-only
          Interval + Wake-system rows appear below, so we don't get
          a stray divider above the Apply button when those rows
          are hidden. The radio cards already have their own outer
          borders, so no separator is needed at the bottom of the
          Mode group itself. */}
      <div
        data-settings-id="wake-scheduler.mode"
        className={`py-2 ${settings.mode === 'heartbeat' ? 'border-b border-[var(--color-border)]' : ''}`}
      >
        <div className="text-xs text-[var(--color-text-secondary)] mb-2">Mode</div>
        <div className="space-y-1">
          {(['off', 'on_demand', 'heartbeat'] as WakeMode[]).map((mode) => (
            <label
              key={mode}
              className={`flex cursor-pointer items-start gap-2 px-2 py-2 border transition-colors no-drag ${
                settings.mode === mode
                  ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/8'
                  : 'border-[var(--color-border)] hover:border-[var(--color-text-secondary)]'
              }`}
            >
              <input
                type="radio"
                name="wake-mode"
                checked={settings.mode === mode}
                onChange={() => update({ mode })}
                className="mt-0.5"
              />
              <div className="flex-1 min-w-0">
                <div className="text-xs text-[var(--color-text-secondary)]">
                  {mode === 'off' && 'Off'}
                  {mode === 'on_demand' && 'On-demand while app open'}
                  {mode === 'heartbeat' && 'Heartbeat every N minutes'}
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                  {MODE_DESCRIPTIONS[mode]}
                </div>
              </div>
            </label>
          ))}
        </div>
      </div>

      {/* Interval — only when mode=heartbeat */}
      {settings.mode === 'heartbeat' && (
        <>
          <div
            data-settings-id="wake-scheduler.interval"
            className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
          >
            <div className="flex-1 min-w-0 mr-3">
              <span className="text-xs text-[var(--color-text-secondary)]">Interval</span>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                Minutes between fires (1–1440). Lower intervals burn more battery; 1–5 minutes balances responsiveness and power.
              </p>
            </div>
            <input
              type="number"
              min={1}
              max={1440}
              value={settings.intervalMinutes}
              onChange={(e) =>
                update({
                  intervalMinutes: Math.max(
                    1,
                    Math.min(1440, parseInt(e.target.value, 10) || 1),
                  ),
                })
              }
              className="w-20 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-primary)] no-drag"
            />
          </div>

          {/* Wake system from sleep */}
          <div
            data-settings-id="wake-scheduler.wake-system"
            className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
          >
            <div className="flex-1 min-w-0 mr-3">
              <span className="text-xs text-[var(--color-text-secondary)]">
                Wake system from sleep
              </span>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                Adds <code className="font-mono text-[var(--color-text-secondary)]">WakeSystem: true</code> to the plist (the same mechanism Time Machine uses for battery-powered hourly backups). When off, scheduled fires run on the next user-initiated wake.
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={settings.wakeSystem}
              onClick={() => update({ wakeSystem: !settings.wakeSystem })}
              className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                settings.wakeSystem
                  ? 'bg-[var(--color-accent)]'
                  : 'bg-[var(--color-border)]'
              }`}
            >
              <span
                className={`w-2.5 h-2.5 bg-white block transition-transform ${
                  settings.wakeSystem ? 'translate-x-3.5' : 'translate-x-0.5'
                }`}
              />
            </button>
          </div>
        </>
      )}

      {/* Apply — no extra border on this row; the preceding setting
          row already carries its own bottom border, so adding a
          top-border here would render as a doubled separator. */}
      <div className="flex items-center gap-3 mt-4">
        <button
          type="button"
          onClick={handleApply}
          disabled={!dirty || applying}
          className="px-3 py-1 text-xs font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity cursor-pointer no-drag disabled:opacity-30 disabled:cursor-not-allowed"
        >
          {applying ? 'Applying…' : 'Apply'}
        </button>
        {dirty && !applying && (
          <span className="text-[10px] text-[var(--color-text-muted)]">Unsaved changes</span>
        )}
      </div>
    </div>
  )
}

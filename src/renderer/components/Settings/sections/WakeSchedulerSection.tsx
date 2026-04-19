import React, { useCallback, useEffect, useState } from 'react'
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
    'No launchd plist. Heartbeats only fire while the K2SO app is open. Agents sit idle when you quit the app.',
  on_demand:
    'Heartbeats fire while the K2SO app is open. The daemon stays running in the background when you quit the app, but the system will not wake itself to run them.',
  heartbeat:
    'launchd fires scheduled heartbeats every N minutes. When "Wake System From Sleep" is on, the laptop wakes from sleep (lid closed, on battery) to run agents — the configuration that makes overnight agent work possible.',
}

export function WakeSchedulerSection(): React.JSX.Element {
  const [loaded, setLoaded] = useState(false)
  const [settings, setSettings] = useState<WakeSchedulerSettings>(DEFAULT_SETTINGS)
  const [dirty, setDirty] = useState(false)
  const [applying, setApplying] = useState(false)
  const toast = useToastStore((s) => s.push)

  // Load settings on mount.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const app = await invoke<AppSettingsShape>('settings_get')
        if (cancelled) return
        const cur = app.wakeScheduler ?? DEFAULT_SETTINGS
        setSettings({
          mode: (cur.mode as WakeMode) ?? 'on_demand',
          intervalMinutes: cur.intervalMinutes ?? 5,
          wakeSystem: cur.wakeSystem ?? false,
        })
      } catch {
        setSettings(DEFAULT_SETTINGS)
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
    setDirty(true)
  }, [])

  const handleApply = useCallback(async () => {
    setApplying(true)
    try {
      // 1. Persist the new settings to settings.json (deep-merge patch).
      await invoke('settings_update', {
        updates: {
          wakeScheduler: {
            mode: settings.mode,
            intervalMinutes: settings.intervalMinutes,
            wakeSystem: settings.wakeSystem,
          },
        },
      })
      // 2. Install/uninstall the launchd plist to match.
      const msg = await invoke<string>('k2so_agents_apply_wake_scheduler')
      toast(msg, 'success')
      setDirty(false)
    } catch (err) {
      toast(`Failed to apply wake scheduler: ${String(err)}`, 'error')
    } finally {
      setApplying(false)
    }
  }, [settings, toast])

  if (!loaded) {
    return <div className="p-6 text-zinc-400">Loading…</div>
  }

  return (
    <div className="max-w-3xl space-y-8 p-6">
      <header>
        <h2 className="mb-1 text-xl font-semibold text-zinc-100">Wake Scheduler</h2>
        <p className="text-sm text-zinc-400">
          Controls how launchd fires heartbeats — and whether it wakes your laptop from sleep to do it.
          Configures <code className="rounded bg-zinc-800 px-1 py-0.5 text-xs">~/Library/LaunchAgents/com.k2so.agent-heartbeat.plist</code>.
        </p>
      </header>

      {/* Mode */}
      <section data-settings-id="wake-scheduler.mode" className="space-y-3">
        <h3 className="text-sm font-semibold uppercase tracking-wide text-zinc-300">Mode</h3>
        <div className="space-y-2">
          {(['off', 'on_demand', 'heartbeat'] as WakeMode[]).map((mode) => (
            <label
              key={mode}
              className={`flex cursor-pointer items-start gap-3 rounded-md border p-3 transition ${
                settings.mode === mode
                  ? 'border-blue-500 bg-blue-500/10'
                  : 'border-zinc-700 hover:border-zinc-600'
              }`}
            >
              <input
                type="radio"
                name="wake-mode"
                checked={settings.mode === mode}
                onChange={() => update({ mode })}
                className="mt-0.5"
              />
              <div>
                <div className="font-medium text-zinc-100">
                  {mode === 'off' && 'Off'}
                  {mode === 'on_demand' && 'On-demand while app open'}
                  {mode === 'heartbeat' && 'Heartbeat every N minutes'}
                </div>
                <div className="mt-1 text-sm text-zinc-400">{MODE_DESCRIPTIONS[mode]}</div>
              </div>
            </label>
          ))}
        </div>
      </section>

      {/* Interval — only when mode=heartbeat */}
      {settings.mode === 'heartbeat' && (
        <>
          <section data-settings-id="wake-scheduler.interval" className="space-y-2">
            <h3 className="text-sm font-semibold uppercase tracking-wide text-zinc-300">
              Interval
            </h3>
            <div className="flex items-center gap-3">
              <input
                type="number"
                min={1}
                max={1440}
                value={settings.intervalMinutes}
                onChange={(e) =>
                  update({
                    intervalMinutes: Math.max(1, Math.min(1440, parseInt(e.target.value, 10) || 1)),
                  })
                }
                className="w-24 rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-zinc-100"
              />
              <span className="text-sm text-zinc-400">minutes between fires (1–1440)</span>
            </div>
            <p className="text-xs text-zinc-500">
              Default: 5. Lower intervals burn more battery on laptops; 5–15 minutes balances
              responsiveness and power.
            </p>
          </section>

          {/* Wake system from sleep */}
          <section data-settings-id="wake-scheduler.wake-system" className="space-y-2">
            <h3 className="text-sm font-semibold uppercase tracking-wide text-zinc-300">
              Wake system from sleep
            </h3>
            <label className="flex cursor-pointer items-start gap-3 rounded-md border border-zinc-700 p-3 hover:border-zinc-600">
              <input
                type="checkbox"
                checked={settings.wakeSystem}
                onChange={(e) => update({ wakeSystem: e.target.checked })}
                className="mt-0.5"
              />
              <div>
                <div className="font-medium text-zinc-100">
                  Allow launchd to wake the laptop from sleep
                </div>
                <div className="mt-1 text-sm text-zinc-400">
                  Adds <code className="rounded bg-zinc-800 px-1 text-xs">WakeSystem: true</code> to
                  the plist. Same mechanism Time Machine uses for battery-powered hourly backups.
                  When off, scheduled fires run on the next user-initiated wake.
                </div>
              </div>
            </label>
          </section>
        </>
      )}

      {/* Apply */}
      <div className="flex items-center gap-3 border-t border-zinc-800 pt-4">
        <button
          type="button"
          onClick={handleApply}
          disabled={!dirty || applying}
          className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-blue-500 disabled:bg-zinc-700 disabled:text-zinc-400"
        >
          {applying ? 'Applying…' : 'Apply'}
        </button>
        {dirty && !applying && (
          <span className="text-xs text-zinc-400">Unsaved changes</span>
        )}
      </div>
    </div>
  )
}

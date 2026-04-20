import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { useUpdateStore } from '@/stores/update'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import { AgenticSystemsToggle } from '../shared/AgenticSystemsToggle'
import { ClaudeAuthRefreshRow } from '../shared/ClaudeAuthRefreshRow'
import { LocalLLMSettings } from '../shared/LocalLLMSettings'
import type { SettingEntry } from '../searchManifest'

export const GENERAL_MANIFEST: SettingEntry[] = [
  { id: 'general.app-version', section: 'general', label: 'App Version', description: 'K2SO version and auto-updater', keywords: ['update', 'version', 'check', 'release'] },
  { id: 'general.cli-version', section: 'general', label: 'CLI Version', description: 'Installed k2so CLI version + install/update button', keywords: ['k2so', 'cli', 'terminal', 'install', 'update', 'path'] },
  { id: 'general.agentic-systems', section: 'general', label: 'Agentic Systems', description: 'Enable AI agent orchestration, workspace manager, heartbeat, review queue', keywords: ['ai', 'agent', 'agentic', 'heartbeat', 'manager', 'workspace states', 'review', 'beta'] },
  { id: 'general.claude-auth-refresh', section: 'general', label: 'Auto-refresh Claude credentials', description: 'Background scheduler that keeps your Claude session alive', keywords: ['claude', 'auth', 'token', 'login', 'credentials', 'scheduler'] },
  { id: 'general.daemon', section: 'general', label: 'K2SO Daemon', description: 'Background service that keeps agents running when the app is closed', keywords: ['daemon', 'background', 'launchd', 'persistent', 'lid', 'sleep', 'wake', 'agent'] },
  { id: 'general.ai-assistant', section: 'general', label: 'AI Workspace Assistant', description: 'Local LLM for natural-language workspace operations (⌘L)', keywords: ['ai', 'assistant', 'llm', 'cmd+l', 'qwen', 'model', 'local', 'gguf'] },
  { id: 'general.model-status', section: 'general', label: 'Model Status', description: 'Current local LLM load state', keywords: ['model', 'llm', 'loaded', 'download'] },
  { id: 'general.download-model', section: 'general', label: 'Download Default Model', description: 'Fetch Qwen2.5-1.5B locally (~1.1GB)', keywords: ['download', 'model', 'qwen', 'local llm'] },
  { id: 'general.custom-model', section: 'general', label: 'Custom Model', description: 'Point at any GGUF model file', keywords: ['model', 'gguf', 'custom', 'load'] },
  { id: 'general.config-location', section: 'general', label: 'Config Location', description: '~/.k2so/settings.json', keywords: ['settings', 'config', 'location', 'path'] },
  { id: 'general.reset-all', section: 'general', label: 'Reset All Settings', description: 'Revert every setting to its default', keywords: ['reset', 'defaults', 'factory'] },
]

export function GeneralSection(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  const [confirming, setConfirming] = useState(false)
  const [currentVersion, setCurrentVersion] = useState<string>('')
  const updateStatus = useUpdateStore((s) => s.status)
  const updateVersion = useUpdateStore((s) => s.version)
  const updateProgress = useUpdateStore((s) => s.progress)
  const updateError = useUpdateStore((s) => s.error)

  // Load current version on mount
  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    await checkForUpdate(true)
  }, [])

  // Auto-check for updates when navigated here from the update toast
  useEffect(() => {
    if (useSettingsStore.getState().pendingUpdateCheck) {
      useSettingsStore.setState({ pendingUpdateCheck: false })
      handleCheckUpdate()
    }
  }, [handleCheckUpdate])

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">General</h2>

      <div className="space-y-4">
        {/* Version & Update */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">App Version</span>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateStatus === 'available' ? '#eab308' : '#4ade80' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{currentVersion || '...'}
              </span>
            </div>
            {updateStatus === 'idle' && (
              <button
                onClick={handleCheckUpdate}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
              >
                Check for Updates
              </button>
            )}
            {updateStatus === 'checking' && (
              <span className="text-[10px] text-[var(--color-text-muted)]">Checking...</span>
            )}
          </div>
        </div>

        {/* Update available */}
        {updateStatus === 'available' && updateVersion && (
          <div className="flex items-center justify-between p-3 bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">K2SO v{updateVersion} is available</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">You&apos;re on v{currentVersion}</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().startDownload()}
            >
              Download & Install
            </button>
          </div>
        )}

        {/* Downloading */}
        {updateStatus === 'downloading' && (
          <div className="p-3 border border-[var(--color-border)]">
            <div className="flex items-center justify-between mb-2">
              <span className="text-xs text-[var(--color-text-primary)]">Downloading v{updateVersion}...</span>
              <span className="text-[10px] tabular-nums text-[var(--color-text-muted)]">{updateProgress}%</span>
            </div>
            <div className="h-1.5 bg-[var(--color-border)] overflow-hidden">
              <div
                className="h-full bg-[var(--color-accent)] transition-all duration-300"
                style={{ width: `${updateProgress}%` }}
              />
            </div>
          </div>
        )}

        {/* Ready to install */}
        {updateStatus === 'ready' && (
          <div className="flex items-center justify-between p-3 bg-green-500/10 border border-green-500/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">v{updateVersion} is ready to install</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">The app will restart after installation</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-green-500 text-white hover:bg-green-600 transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().installAndRelaunch()}
            >
              Install & Relaunch
            </button>
          </div>
        )}

        {/* Error */}
        {updateStatus === 'error' && (
          <div className="p-3 border border-red-500/30 bg-red-500/5">
            <p className="text-[10px] text-red-400">{updateError}</p>
            <div className="flex items-center gap-2 mt-2">
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                onClick={handleCheckUpdate}
              >
                Retry
              </button>
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
                onClick={() => {
                  const tag = updateVersion ? `v${updateVersion}` : 'latest'
                  invoke('plugin:opener|open_url', { url: `https://github.com/Alakazam-211/K2SO/releases/tag/${tag}` }).catch(() => {
                    window.open(`https://github.com/Alakazam-211/K2SO/releases/tag/${tag}`)
                  })
                }}
              >
                Download
              </button>
            </div>
          </div>
        )}

        {/* CLI Version — right under App Version so it feels like part of the app */}
        <CLIVersionRow />

        {/* Agentic Systems master switch */}
        <AgenticSystemsToggle />

        {/* Claude Auth Auto-Refresh */}
        <ClaudeAuthRefreshRow />

        {/* K2SO Daemon — persistent-agents service */}
        <DaemonRow />

        {/* AI Workspace Assistant (Cmd+L) — core feature, belongs in General */}
        <LocalLLMSettings />

        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Config Location</span>
          <span className="text-xs text-[var(--color-text-muted)]">~/.k2so/settings.json</span>
        </div>

        <div className="pt-4">
          {confirming ? (
            <div className="flex items-center gap-2">
              <span className="text-xs text-red-400">Reset all settings to defaults?</span>
              <button
                onClick={() => {
                  resetAllSettings()
                  setConfirming(false)
                }}
                className="px-3 py-1 text-xs bg-red-500/20 text-red-400 border border-red-500/40 hover:bg-red-500/30 no-drag cursor-pointer"
              >
                Confirm
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              className="px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer"
            >
              Reset All Settings
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

function CLIVersionRow(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    installedVersion: string | null
    bundledVersion: string | null
    updateAvailable: boolean
  } | null>(null)
  const [loading, setLoading] = useState(false)
  const [checking, setChecking] = useState(false)

  const checkStatus = useCallback(async () => {
    try {
      const result = await invoke<{
        installed: boolean
        installedVersion: string | null
        bundledVersion: string | null
        updateAvailable: boolean
      }>('cli_install_status')
      setStatus(result)
    } catch {
      // silently fail
    }
  }, [])

  useEffect(() => { checkStatus() }, [checkStatus])

  const handleInstallOrUpdate = useCallback(async () => {
    setLoading(true)
    try {
      await invoke('cli_install')
      await checkStatus()
    } catch (err) {
      console.error('[cli]', err)
    } finally {
      setLoading(false)
    }
  }, [checkStatus])

  const handleCheckForUpdates = useCallback(async () => {
    setChecking(true)
    try {
      await checkStatus()
    } finally {
      setChecking(false)
    }
  }, [checkStatus])

  // Compare versions properly — only show update if bundled is actually newer
  const compareVersions = (a: string, b: string): number => {
    const pa = a.split('.').map(Number)
    const pb = b.split('.').map(Number)
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const va = pa[i] || 0
      const vb = pb[i] || 0
      if (va > vb) return 1
      if (va < vb) return -1
    }
    return 0
  }
  const updateAvailable = status?.installed && status.bundledVersion && status.installedVersion
    && compareVersions(status.bundledVersion, status.installedVersion) > 0

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">CLI Version</span>
      <div className="flex items-center gap-3">
        {status?.installed ? (
          <>
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateAvailable ? '#eab308' : '#4ade80' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{status.installedVersion || '?'}
              </span>
            </div>
            {updateAvailable ? (
              <button
                onClick={handleInstallOrUpdate}
                disabled={loading}
                className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Updating...' : `Update to v${status.bundledVersion}`}
              </button>
            ) : (
              <button
                onClick={handleCheckForUpdates}
                disabled={checking}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                {checking ? 'Checking...' : 'Check for Updates'}
              </button>
            )}
          </>
        ) : (
          <>
            <span className="text-xs text-[var(--color-text-muted)]">Not installed</span>
            <button
              onClick={handleInstallOrUpdate}
              disabled={loading}
              className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {loading ? 'Installing...' : 'Install'}
            </button>
          </>
        )}
      </div>
    </div>
  )
}

// ── K2SO Daemon ────────────────────────────────────────────────────────
// Backs the persistent-agents feature: a launchd-managed background
// process that keeps agents running while the Tauri window is closed
// and (optionally) wakes the machine from sleep on a schedule. This
// row is how a user knows it's running, installs it, or turns it off.
//
// The shape returned by `daemon_status` is a tagged union — we dispatch
// on `state` to decide which action buttons to show. Every button is a
// thin wrapper over a Tauri command; the command handlers wrap
// k2so_core::wake / launchctl. The frontend never touches launchctl
// directly.

type DaemonStatusState =
  | { state: 'running'; version: string; uptime_secs: number; pid: number; port: number }
  | { state: 'not_installed'; reason: string }
  | { state: 'unreachable'; reason: string }

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`
}

function DaemonRow(): React.JSX.Element {
  const [status, setStatus] = useState<DaemonStatusState | null>(null)
  const [busy, setBusy] = useState<null | 'install' | 'uninstall' | 'restart'>(null)
  const [error, setError] = useState<string | null>(null)
  const [confirmingUninstall, setConfirmingUninstall] = useState(false)
  const [showingLog, setShowingLog] = useState(false)
  const [logText, setLogText] = useState<string>('')

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<DaemonStatusState>('daemon_status')
      setStatus(result)
      setError(null)
    } catch (e) {
      setError(String(e))
    }
  }, [])

  // Refresh on mount + every 4s while the row is visible. Cheap —
  // daemon_status is just a file read + an HTTP ping on localhost.
  useEffect(() => {
    refresh()
    const id = window.setInterval(refresh, 4000)
    return () => window.clearInterval(id)
  }, [refresh])

  const handleInstall = useCallback(async () => {
    setBusy('install')
    setError(null)
    try {
      await invoke<string>('daemon_install')
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleUninstall = useCallback(async () => {
    setBusy('uninstall')
    setError(null)
    try {
      await invoke('daemon_uninstall')
      setConfirmingUninstall(false)
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleRestart = useCallback(async () => {
    setBusy('restart')
    setError(null)
    try {
      await invoke('daemon_restart')
      // Give launchd a moment to respawn before we query again.
      await new Promise((r) => setTimeout(r, 1200))
      await refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(null)
    }
  }, [refresh])

  const handleViewLog = useCallback(async () => {
    try {
      const text = await invoke<string>('daemon_log_tail', { lines: 300 })
      setLogText(text)
      setShowingLog(true)
    } catch (e) {
      setError(String(e))
    }
  }, [])

  const dotColor = (() => {
    if (!status) return '#6b7280'                          // loading
    if (status.state === 'running') return '#4ade80'       // green
    if (status.state === 'not_installed') return '#6b7280' // neutral
    return '#eab308'                                        // yellow (unreachable)
  })()

  const statusText = (() => {
    if (!status) return 'Loading...'
    if (status.state === 'running') {
      return `Running (PID ${status.pid}, up ${formatUptime(status.uptime_secs)})`
    }
    if (status.state === 'not_installed') return 'Not installed'
    return 'Installed but unreachable'
  })()

  return (
    <div className="py-2 border-b border-[var(--color-border)]">
      <div className="flex items-center justify-between">
        <div className="flex flex-col">
          <span className="text-xs text-[var(--color-text-secondary)]">K2SO Daemon</span>
          <span className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Keeps agents running when the app is closed
          </span>
        </div>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-1.5">
            <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: dotColor }} />
            <span className="text-xs text-[var(--color-text-muted)]">{statusText}</span>
          </div>
          {status?.state === 'not_installed' && (
            <button
              onClick={handleInstall}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'install' ? 'Installing...' : 'Install'}
            </button>
          )}
          {status?.state === 'running' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'restart' ? 'Restarting...' : 'Restart'}
            </button>
          )}
          {status?.state === 'unreachable' && (
            <button
              onClick={handleRestart}
              disabled={busy !== null}
              className="px-2 py-0.5 text-[10px] bg-yellow-500/20 text-yellow-400 border border-yellow-500/30 hover:bg-yellow-500/30 transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {busy === 'restart' ? 'Restarting...' : 'Restart'}
            </button>
          )}
        </div>
      </div>

      {/* Secondary row — log + uninstall affordances only shown when installed */}
      {status && status.state !== 'not_installed' && (
        <div className="flex items-center gap-3 mt-1.5 pl-0">
          <button
            onClick={handleViewLog}
            className="text-[10px] text-[var(--color-text-muted)] underline hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            View log
          </button>
          {confirmingUninstall ? (
            <>
              <span className="text-[10px] text-red-400">Remove the launch agent?</span>
              <button
                onClick={handleUninstall}
                disabled={busy !== null}
                className="px-2 py-0.5 text-[10px] bg-red-500/20 text-red-400 border border-red-500/40 hover:bg-red-500/30 no-drag cursor-pointer disabled:opacity-50"
              >
                {busy === 'uninstall' ? 'Removing...' : 'Confirm'}
              </button>
              <button
                onClick={() => setConfirmingUninstall(false)}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Cancel
              </button>
            </>
          ) : (
            <button
              onClick={() => setConfirmingUninstall(true)}
              className="text-[10px] text-[var(--color-text-muted)] underline hover:text-red-400 transition-colors no-drag cursor-pointer"
            >
              Uninstall daemon
            </button>
          )}
        </div>
      )}

      {/* Inline log viewer — appears when "View log" is clicked */}
      {showingLog && (
        <div className="mt-2 p-2 bg-black/30 border border-[var(--color-border)] max-h-60 overflow-auto">
          <div className="flex items-center justify-between mb-1">
            <span className="text-[10px] text-[var(--color-text-muted)]">~/.k2so/daemon.stdout.log (last 300 lines)</span>
            <button
              onClick={() => setShowingLog(false)}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              Close
            </button>
          </div>
          <pre className="text-[10px] text-[var(--color-text-muted)] whitespace-pre-wrap font-mono leading-tight">
            {logText || '(log file empty or missing)'}
          </pre>
        </div>
      )}

      {/* Error surface — visible until the next action clears it */}
      {error && (
        <div className="mt-2 p-2 bg-red-500/5 border border-red-500/30">
          <p className="text-[10px] text-red-400 break-all">{error}</p>
        </div>
      )}
    </div>
  )
}

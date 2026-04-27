import React from 'react'
import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { SettingEntry } from '../searchManifest'

export const COMPANION_MANIFEST: SettingEntry[] = [
  { id: 'companion.enable', section: 'companion', label: 'Enable Companion', description: 'Start the ngrok tunnel for the K2SO mobile companion app', keywords: ['mobile', 'companion', 'remote', 'ngrok', 'tunnel'] },
  { id: 'companion.auto-start', section: 'companion', label: 'Start on Launch', description: 'Auto-connect when K2SO opens', keywords: ['auto', 'launch', 'startup'] },
  { id: 'companion.allow-remote-spawn', section: 'companion', label: 'Allow Remote Spawn', description: 'Permit mobile app to launch arbitrary terminals (off by default)', keywords: ['spawn', 'terminal', 'command', 'security', 'remote', 'execute'] },
  { id: 'companion.username', section: 'companion', label: 'Username', description: 'Username the mobile app authenticates with', keywords: ['username', 'auth', 'login'] },
  { id: 'companion.password', section: 'companion', label: 'Password', description: 'Password the mobile app authenticates with', keywords: ['password', 'auth', 'login'] },
  { id: 'companion.ngrok-token', section: 'companion', label: 'ngrok Auth Token', description: 'Required for the remote tunnel', keywords: ['ngrok', 'token', 'tunnel', 'auth'] },
  { id: 'companion.ngrok-domain', section: 'companion', label: 'Custom Domain', description: 'Paid ngrok plans (e.g. myapp.ngrok.app)', keywords: ['domain', 'ngrok', 'url'] },
  { id: 'companion.sessions', section: 'companion', label: 'Active Sessions', description: 'Connected mobile companion clients', keywords: ['sessions', 'connected', 'disconnect', 'client'] },
]

export function CompanionSection(): React.JSX.Element {
  const [enabled, setEnabled] = useState(false)
  const [autoStart, setAutoStart] = useState(false)
  const [allowRemoteSpawn, setAllowRemoteSpawn] = useState(false)
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [passwordSet, setPasswordSet] = useState(false)
  const [ngrokToken, setNgrokToken] = useState('')
  const [ngrokDomain, setNgrokDomain] = useState('')
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null)
  const [connectedClients, setConnectedClients] = useState(0)
  const [sessions, setSessions] = useState<Array<{ token: string; remoteAddr: string; createdAt: string }>>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [urlCopied, setUrlCopied] = useState(false)

  useEffect(() => {
    const load = async () => {
      try {
        const settings = await invoke<any>('settings_get')
        const c = settings?.companion || {}
        setUsername(c.username || '')
        // Post-0.32.12 the hash itself moves to Keychain and passwordHash is
        // blanked; `passwordSet` is the durable indicator. Fall back to the
        // legacy field for installs that haven't migrated yet.
        setPasswordSet(!!(c.passwordSet || c.passwordHash))
        setNgrokToken(c.ngrokAuthToken || '')
        setNgrokDomain(c.ngrokDomain || '')
        setAutoStart(c.autoStart || false)
        setAllowRemoteSpawn(!!c.allowRemoteSpawn)
      } catch { /* ignore */ }
      try {
        const status = await invoke<any>('companion_status')
        if (status.running) {
          setEnabled(true)
          if (status.tunnelUrl) {
            setTunnelUrl(status.tunnelUrl)
            setConnectedClients(status.connectedClients || 0)
            setSessions(status.sessions || [])
          }
        } else {
          setEnabled(false)
        }
      } catch { /* ignore */ }
    }
    load()
  }, [])

  useEffect(() => {
    // Poll companion status — runs always when autoStart is on (to detect auto-started companion)
    // or when enabled (to track running companion)
    if (!enabled && !autoStart) return
    const interval = setInterval(async () => {
      try {
        const status = await invoke<any>('companion_status')
        if (!status.running) {
          if (enabled) {
            // Tunnel genuinely stopped
            setEnabled(false)
            setTunnelUrl(null)
            setConnectedClients(0)
            setSessions([])
          }
        } else {
          // Companion is running — make sure UI reflects it
          if (!enabled) setEnabled(true)
          if (status.tunnelUrl) {
            setTunnelUrl(status.tunnelUrl)
            setConnectedClients(status.connectedClients || 0)
            setSessions(status.sessions || [])
          }
        }
      } catch { /* ignore */ }
    }, 5000)
    return () => clearInterval(interval)
  }, [enabled, autoStart])

  const handleToggle = async () => {
    setLoading(true)
    setError(null)
    try {
      if (enabled) {
        await invoke('companion_stop')
        setEnabled(false)
        setTunnelUrl(null)
        setConnectedClients(0)
        setSessions([])
        await invoke('settings_update', { updates: { companion: { enabled: false } } })
      } else {
        await invoke('settings_update', {
          updates: { companion: { enabled: true, username, ngrokAuthToken: ngrokToken } }
        })
        const url = await invoke<string>('companion_start')
        setEnabled(true)
        setTunnelUrl(url)
      }
    } catch (err: any) {
      setError(typeof err === 'string' ? err : err?.message || 'Failed')
    } finally {
      setLoading(false)
    }
  }

  const handleSetPassword = async () => {
    if (!password) return
    try {
      await invoke('companion_set_password', { password })
      setPasswordSet(true)
      setPassword('')
    } catch (err: any) {
      setError(typeof err === 'string' ? err : 'Failed to set password')
    }
  }

  const handleDisconnect = async (token: string) => {
    try {
      await invoke('companion_disconnect_session', { sessionToken: token })
      setSessions((prev) => prev.filter((s) => s.token !== token))
    } catch { /* ignore */ }
  }

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        Mobile Companion
        <span
          className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
          title="This feature is in beta — interface and behavior may change"
        >
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
        Access your K2SO agents remotely through the companion app. Requires an ngrok account.
      </p>

      {/* Deprecation notice — the mobile app pairing surface predates
          the daemon-first / multi-heartbeat architecture and needs a
          rewrite against the new session model before it can ship
          again. The settings here still work for users on 0.29.x who
          have a paired companion, but new pairings won't connect
          against current K2SO builds. */}
      <div className="flex items-start gap-2 mb-4 px-3 py-2 border border-amber-400/30 bg-amber-400/5">
        <span className="text-amber-400 text-sm leading-none flex-shrink-0 mt-0.5">&#9888;</span>
        <div className="text-[10px] text-amber-300/80 leading-relaxed">
          <strong className="text-amber-300">Mobile app support paused.</strong>{' '}
          K2SO <span className="font-mono">0.29.x</span> is the last version that
          fully supports the mobile companion app. The settings on this page still
          work for ngrok tunnel + auth setup, but the mobile app itself isn&apos;t
          compatible with the current K2SO session model. Full mobile support is
          coming back in a later release.
        </div>
      </div>

      <div className="flex items-center gap-2 mb-4 px-3 py-2 border border-[var(--color-border)]">
        <span className="w-2 h-2 flex-shrink-0" style={{ backgroundColor: tunnelUrl ? '#22c55e' : enabled ? '#eab308' : '#6b7280' }} />
        <span className="text-xs text-[var(--color-text-secondary)]">
          {tunnelUrl ? `Connected (${connectedClients} client${connectedClients !== 1 ? 's' : ''})` : enabled ? 'Connecting...' : 'Not running'}
        </span>
        {tunnelUrl && (
          <div className="flex items-center gap-1.5 ml-auto">
            <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate max-w-[200px]">{tunnelUrl}</span>
            <button
              onClick={() => {
                navigator.clipboard.writeText(tunnelUrl).then(() => {
                  setUrlCopied(true)
                  setTimeout(() => setUrlCopied(false), 1500)
                }).catch(() => {})
              }}
              className={`text-[10px] no-drag cursor-pointer ${urlCopied ? 'text-green-400' : 'text-[var(--color-accent)] hover:underline'}`}
            >
              {urlCopied ? 'Copied!' : 'Copy'}
            </button>
          </div>
        )}
      </div>

      {error && <div className="text-[10px] text-red-400 mb-3 px-3 py-1.5 border border-red-400/20 bg-red-400/5">{error}</div>}

      <div className="space-y-0">
        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Enable Companion</span>
          <button onClick={handleToggle} disabled={loading} className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'} ${loading ? 'opacity-50' : ''}`}>
            <span className={`w-2.5 h-2.5 bg-white block transition-transform ${enabled ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Start on Launch</span>
            <p className="text-[10px] text-[var(--color-text-muted)]">Automatically connect when K2SO opens</p>
          </div>
          <button
            onClick={() => {
              const next = !autoStart
              setAutoStart(next)
              invoke('settings_update', { updates: { companion: { autoStart: next } } }).catch(() => {})
            }}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${autoStart ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'}`}
          >
            <span className={`w-2.5 h-2.5 bg-white block transition-transform ${autoStart ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Allow Remote Spawn</span>
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Permit the mobile app to launch new terminals running arbitrary commands.
              Off by default — if the tunnel is compromised, leaving this off limits the
              blast radius to reading existing terminals. Restart the companion after changing.
            </p>
          </div>
          <button
            onClick={() => {
              const next = !allowRemoteSpawn
              setAllowRemoteSpawn(next)
              invoke('settings_update', { updates: { companion: { allowRemoteSpawn: next } } }).catch(() => {})
            }}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${allowRemoteSpawn ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'}`}
          >
            <span className={`w-2.5 h-2.5 bg-white block transition-transform ${allowRemoteSpawn ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Username</span>
          <input type="text" value={username} onChange={(e) => setUsername(e.target.value)} onBlur={() => invoke('settings_update', { updates: { companion: { username } } }).catch(() => {})} placeholder="Enter username" className="w-48 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Password</span>
            {passwordSet && <span className="ml-2 text-[10px] text-green-400">Set</span>}
          </div>
          <div className="flex items-center gap-1.5">
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') handleSetPassword() }} placeholder={passwordSet ? '••••••••' : 'Enter password'} className="w-36 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
            {password && <button onClick={handleSetPassword} className="px-2 py-1 text-[10px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer">Save</button>}
          </div>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">ngrok Auth Token</span>
          <input type="password" value={ngrokToken} onChange={(e) => setNgrokToken(e.target.value)} onBlur={() => invoke('settings_update', { updates: { companion: { ngrokAuthToken: ngrokToken } } }).catch(() => {})} placeholder="Enter ngrok token" className="w-48 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Custom Domain</span>
            <p className="text-[10px] text-[var(--color-text-muted)]">Paid plans only (e.g. myapp.ngrok.app)</p>
          </div>
          <input type="text" value={ngrokDomain} onChange={(e) => setNgrokDomain(e.target.value)} onBlur={() => invoke('settings_update', { updates: { companion: { ngrokDomain: ngrokDomain } } }).catch(() => {})} placeholder="Optional" className="w-48 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
        </div>
      </div>

      {sessions.length > 0 && (
        <div className="mt-6">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">Active Sessions</h3>
          <div className="border border-[var(--color-border)]">
            {sessions.map((session) => (
              <div key={session.token} className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
                <div className="flex items-center gap-2">
                  <span className="w-1.5 h-1.5 bg-green-400 flex-shrink-0" />
                  <span className="text-xs text-[var(--color-text-primary)] font-mono">{session.remoteAddr}</span>
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    {(() => { const ago = Math.floor((Date.now() - new Date(session.createdAt).getTime()) / 60000); return ago < 1 ? 'just now' : ago < 60 ? `${ago}m ago` : `${Math.floor(ago / 60)}h ago` })()}
                  </span>
                </div>
                <button onClick={() => handleDisconnect(session.token)} className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer">Disconnect</button>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="mt-6 text-[10px] text-[var(--color-text-muted)] space-y-1">
        <p>1. Create a <span className="text-[var(--color-accent)]">paid</span> account at <span className="text-[var(--color-accent)]">ngrok.com</span> (Personal plan or higher) and copy your auth token.</p>
        <p>2. Set a username and password for the companion app to authenticate.</p>
        <p>3. Enable the toggle — K2SO will create a secure tunnel and show you the URL.</p>
        <p>4. Enter the URL in the K2SO companion app on your phone.</p>
        <p className="text-[var(--color-text-muted)] opacity-70 mt-2">A paid ngrok account is required for a stable connection. Free tier tunnels disconnect after a short period of inactivity.</p>
      </div>
    </div>
  )
}

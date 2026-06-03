// Settings → Connections — the K2 Connect CLIENT address book (PRD §1,
// build order step #3). Where users set up / edit / remove the K2 servers
// they connect OUT to. (The OTHER direction — exposing THIS device's own
// daemon — lives in Settings → K2 Connect.)
//
// Each saved ConnectHost: label · address · status dot, with Add / Edit /
// Remove and a "Remember password" toggle. Non-secret fields persist to
// ~/.k2so/connect-hosts.json (via the store's connect_hosts_write); the
// token goes to the OS keychain ONLY when "Remember password" is on, else
// it's kept in memory for the session.
//
// Selecting/connecting a host reuses the top-bar switcher path
// (`pickHost`) so a host without a remembered token drops into the same
// full-screen sign-in.

import React, { useState } from 'react'
import {
  useConnectHostStore,
  isLocalHostname,
  rememberToken,
  forgetToken,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'
import { validateConnectHost } from '@/lib/connect-validate'
import type { SettingEntry } from '../searchManifest'

export const CONNECTIONS_MANIFEST: SettingEntry[] = [
  { id: 'connections.add', section: 'connections', label: 'Add a Server', description: 'Save a remote K2 daemon to connect to', keywords: ['server', 'remote', 'connect', 'host', 'add', 'k2 connect', 'address book'] },
  { id: 'connections.remember-password', section: 'connections', label: 'Remember Password', description: 'Store a server token in your OS keychain', keywords: ['token', 'password', 'keychain', 'remember', 'credentials'] },
  { id: 'connections.list', section: 'connections', label: 'Saved Servers', description: 'Edit or remove saved K2 servers', keywords: ['servers', 'hosts', 'edit', 'remove', 'list'] },
]

function statusColor(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return '#3fb950'
    case 'connecting':
      return '#d29922'
    case 'offline':
      return '#f85149'
  }
}

type DraftHost = {
  id: string | null // null = creating a new host
  label: string
  hostname: string
  port: string
  token: string
  secure: boolean
  remember: boolean
}

function emptyDraft(): DraftHost {
  return { id: null, label: '', hostname: '', port: '', token: '', secure: false, remember: false }
}

export function ConnectionsSection(): React.JSX.Element {
  const hosts = useConnectHostStore((s) => s.hosts)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const addHost = useConnectHostStore((s) => s.addHost)
  const removeHost = useConnectHostStore((s) => s.removeHost)
  const pickHost = useConnectHostStore((s) => s.pickHost)

  const [draft, setDraft] = useState<DraftHost | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [secureTouched, setSecureTouched] = useState(false)
  const [portTouched, setPortTouched] = useState(false)

  const beginAdd = (): void => {
    setError(null)
    setSecureTouched(false)
    setPortTouched(false)
    setDraft(emptyDraft())
  }

  const beginEdit = (h: ConnectHost): void => {
    setError(null)
    setSecureTouched(true)
    setPortTouched(true)
    setDraft({
      id: h.id,
      label: h.label,
      hostname: h.hostname,
      port: String(h.port),
      // We never read the token back out of the keychain into a form
      // field; leave it blank. An empty token on save = "leave the
      // remembered token as-is" (see save()).
      token: '',
      secure: h.secure,
      remember: h.remember,
    })
  }

  // Re-derive secure/port defaults from the hostname unless overridden:
  // non-local → secure + 443; local/LAN → plain.
  const onHostnameChange = (next: string): void => {
    if (!draft) return
    const local = isLocalHostname(next)
    setDraft({
      ...draft,
      hostname: next,
      secure: secureTouched ? draft.secure : !local,
      port: portTouched ? draft.port : !local ? '443' : draft.port,
    })
  }

  const save = async (): Promise<void> => {
    if (!draft) return
    const portNum = Number(draft.port)
    if (!draft.label.trim()) {
      setError('Label is required')
      return
    }
    if (!draft.hostname.trim()) {
      setError('Hostname is required')
      return
    }
    if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
      setError('Port must be 1–65535')
      return
    }

    const id = draft.id ?? `host-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    const existing = draft.id ? hosts.find((h) => h.id === draft.id) : undefined
    const tokenEntered = draft.token.trim()

    // Validate ONLY when a token was entered (a blank token on an edit
    // keeps the existing remembered one untouched — no re-validation).
    if (tokenEntered) {
      setBusy(true)
      setError(null)
      const candidate: ConnectHost = {
        id,
        label: draft.label.trim(),
        hostname: draft.hostname.trim(),
        port: portNum,
        token: tokenEntered,
        secure: draft.secure,
        remember: draft.remember,
        lastConnectedAt: existing?.lastConnectedAt ?? null,
      }
      const result = await validateConnectHost(candidate, tokenEntered)
      setBusy(false)
      if (!result.ok) {
        setError(result.reason)
        return
      }
    }

    const host: ConnectHost = {
      id,
      label: draft.label.trim(),
      hostname: draft.hostname.trim(),
      port: portNum,
      // Keep the entered token in memory; if blank on edit, preserve the
      // existing in-memory token so we don't blank an active connection.
      token: tokenEntered || existing?.token || '',
      secure: draft.secure,
      remember: draft.remember,
      lastConnectedAt: existing?.lastConnectedAt ?? null,
    }
    addHost(host)

    // Keychain side: remember writes (only when we actually have a token
    // to store); un-remember forgets it.
    if (draft.remember && (tokenEntered || host.token)) {
      await rememberToken(id, tokenEntered || host.token)
    } else if (!draft.remember) {
      await forgetToken(id)
    }

    setDraft(null)
    setError(null)
  }

  const connect = (h: ConnectHost): void => {
    pickHost(h)
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Connections</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        K2 servers this device connects to. Each server&apos;s password is stored in your OS
        keychain only when &ldquo;Remember password&rdquo; is on — never in plain text.
      </p>

      {/* Local — always present, never editable. */}
      <div className="flex items-center gap-2 mb-2 px-3 py-2 border border-[var(--color-border)]">
        <span
          className="w-2 h-2 flex-shrink-0 rounded-full"
          style={{ backgroundColor: activeHost === 'local' ? statusColor(connectionStatus) : '#6b7280' }}
        />
        <div className="flex flex-col min-w-0">
          <span className="text-xs text-[var(--color-text-primary)]">Local</span>
          <span className="text-[10px] text-[var(--color-text-muted)]">This Mac · bundled daemon</span>
        </div>
        {activeHost === 'local' ? (
          <span className="ml-auto text-[10px] text-[var(--color-text-muted)]">Active</span>
        ) : (
          <button
            onClick={() => pickHost('local')}
            className="ml-auto text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer"
          >
            Connect
          </button>
        )}
      </div>

      {/* Saved hosts */}
      <div className="space-y-2" data-settings-id="connections.list">
        {hosts.map((h) => {
          const isActive = activeHost !== 'local' && activeHost.id === h.id
          return (
            <div key={h.id} className="px-3 py-2 border border-[var(--color-border)]">
              <div className="flex items-center gap-2">
                <span
                  className="w-2 h-2 flex-shrink-0 rounded-full"
                  style={{ backgroundColor: isActive ? statusColor(connectionStatus) : '#6b7280' }}
                />
                <div className="flex flex-col min-w-0">
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{h.label}</span>
                  <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                    {h.secure ? '🔒 ' : ''}
                    {h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`}
                    {h.remember ? ' · saved' : ''}
                  </span>
                </div>
                <div className="ml-auto flex items-center gap-2">
                  {isActive ? (
                    <span className="text-[10px] text-[var(--color-text-muted)]">Active</span>
                  ) : (
                    <button onClick={() => connect(h)} className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer">
                      Connect
                    </button>
                  )}
                  <button onClick={() => beginEdit(h)} className="text-[10px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer">
                    Edit
                  </button>
                  <button onClick={() => removeHost(h.id)} className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer">
                    Remove
                  </button>
                </div>
              </div>
            </div>
          )
        })}
        {hosts.length === 0 && (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">No saved servers yet.</div>
        )}
      </div>

      {/* Add / Edit form */}
      {draft ? (
        <div className="mt-4 px-3 py-3 border border-[var(--color-border)] space-y-2" data-settings-id="connections.add">
          <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
            {draft.id ? 'Edit server' : 'Add server'}
          </div>
          <input className={inputCls} placeholder="Label (e.g. Hetzner box)" value={draft.label} onChange={(e) => setDraft({ ...draft, label: e.target.value })} />
          <div className="flex gap-2">
            <input className={inputCls} placeholder="hostname (e.g. rosson.k2.dev)" value={draft.hostname} onChange={(e) => onHostnameChange(e.target.value)} />
            <input
              className={inputCls}
              style={{ maxWidth: 80 }}
              placeholder="port"
              value={draft.port}
              onChange={(e) => {
                setPortTouched(true)
                setDraft({ ...draft, port: e.target.value })
              }}
            />
          </div>
          <input
            className={inputCls}
            type="password"
            placeholder={draft.id ? 'Password (leave blank to keep saved)' : 'Password / token'}
            value={draft.token}
            onChange={(e) => setDraft({ ...draft, token: e.target.value })}
          />
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]" data-settings-id="connections.remember-password">
            <input
              type="checkbox"
              checked={draft.secure}
              onChange={(e) => {
                setSecureTouched(true)
                setDraft({ ...draft, secure: e.target.checked })
              }}
            />
            Secure (TLS)
          </label>
          <label className="flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
            <input type="checkbox" checked={draft.remember} onChange={(e) => setDraft({ ...draft, remember: e.target.checked })} />
            Remember password (OS keychain)
          </label>
          {error && <div className="text-[10px] text-red-400">{error}</div>}
          <div className="flex gap-2 pt-1">
            <button
              onClick={() => void save()}
              disabled={busy}
              className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
            >
              {busy ? 'Verifying…' : 'Save'}
            </button>
            <button onClick={() => { setDraft(null); setError(null) }} className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer">
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button onClick={beginAdd} className="mt-4 px-3 py-1.5 text-[11px] text-[var(--color-accent)] border border-[var(--color-accent)]/40 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer">
          + Add a server
        </button>
      )}
    </div>
  )
}

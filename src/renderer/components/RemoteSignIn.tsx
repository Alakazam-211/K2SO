// RemoteSignIn — the full-screen sign-in for a saved K2 server that has
// no remembered token (or a rejected/expired one). PRD §1:
//
//   "Token not remembered (or remembered-but-rejected/expired) →
//    full-screen sign-in for that specific server (pre-filled label +
//    address, focus the password field), then connect."
//
// Mounted by ConnectionGate when `pendingSignIn` is set on the
// connect-host store. On success it:
//   - validates host+token via /boot-status (validateHost)
//   - sets the in-memory token on the host (setHostToken)
//   - if "Remember password" → writes the token to the OS keychain
//     (rememberToken) and persists remember:true; else forgets it
//   - selectHost(host) to commit the switch (gate re-polls + mounts)
//
// Remembered hosts never reach here — they auto-sign-in silently.

import React, { useEffect, useRef, useState } from 'react'
import {
  useConnectHostStore,
  rememberToken,
  forgetToken,
  type ConnectHost,
} from '@/stores/connect-host'
import { validateConnectHost } from '@/lib/connect-validate'

export function RemoteSignIn({ host }: { host: ConnectHost }): React.JSX.Element {
  const selectHost = useConnectHostStore((s) => s.selectHost)
  const addHost = useConnectHostStore((s) => s.addHost)
  const setHostToken = useConnectHostStore((s) => s.setHostToken)
  const cancelSignIn = useConnectHostStore((s) => s.cancelSignIn)

  const [token, setToken] = useState('')
  // Default the remember toggle to the host's saved intent.
  const [remember, setRemember] = useState(host.remember)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const passwordRef = useRef<HTMLInputElement | null>(null)

  // Focus the password field on mount (PRD: "focus the password field").
  useEffect(() => {
    passwordRef.current?.focus()
  }, [])

  const address = host.secure && host.port === 443
    ? host.hostname
    : `${host.hostname}:${host.port}`

  const submit = async (): Promise<void> => {
    if (!token.trim()) {
      setError('Enter the server password / token.')
      return
    }
    setBusy(true)
    setError(null)
    const result = await validateConnectHost(host, token.trim())
    if (!result.ok) {
      setError(result.reason)
      setBusy(false)
      return
    }
    // Commit the token into the in-memory host so daemon-ws.ts uses it.
    setHostToken(host.id, token.trim())
    // Persist remember intent + the keychain side.
    const updated: ConnectHost = {
      ...host,
      token: token.trim(),
      remember,
      lastConnectedAt: Date.now(),
    }
    addHost(updated) // re-persists the (token-less) list with remember + lastConnectedAt
    if (remember) {
      await rememberToken(host.id, token.trim())
    } else {
      await forgetToken(host.id)
    }
    // Switch — the gate re-polls against the new host and mounts on accept.
    selectHost(updated)
  }

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 10000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--color-bg, #0a0a0a)',
        color: 'var(--color-text-primary, #e0e0e0)',
        fontFamily: 'system-ui, -apple-system, sans-serif',
      }}
    >
      <form
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
        style={{
          width: 360,
          maxWidth: '90vw',
          display: 'flex',
          flexDirection: 'column',
          gap: 14,
          padding: 28,
          border: '1px solid var(--color-border, rgba(255,255,255,0.12))',
          borderRadius: 8,
          background: 'var(--color-bg-surface, rgba(255,255,255,0.02))',
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <div style={{ fontSize: '0.95rem', fontWeight: 600 }}>
            Sign in to {host.label}
          </div>
          <div style={{ fontSize: '0.78rem', opacity: 0.65, fontFamily: 'ui-monospace, monospace' }}>
            {host.secure ? '🔒 ' : ''}{address}
          </div>
        </div>

        <label style={{ display: 'flex', flexDirection: 'column', gap: 5, fontSize: '0.78rem', opacity: 0.85 }}>
          Password
          <input
            ref={passwordRef}
            type="password"
            value={token}
            disabled={busy}
            onChange={(e) => setToken(e.target.value)}
            placeholder="Server token / password"
            autoComplete="off"
            style={{
              padding: '0.5rem 0.65rem',
              fontSize: '0.85rem',
              borderRadius: 4,
              border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
              background: 'var(--color-bg, rgba(0,0,0,0.3))',
              color: 'inherit',
              outline: 'none',
            }}
          />
        </label>

        <label style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: '0.78rem', opacity: 0.85, cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={remember}
            disabled={busy}
            onChange={(e) => setRemember(e.target.checked)}
          />
          Remember password (stored in your OS keychain)
        </label>

        {error && (
          <div style={{ fontSize: '0.75rem', color: '#f85149' }} role="alert">
            {error}
          </div>
        )}

        <div style={{ display: 'flex', gap: 8, marginTop: 4 }}>
          <button
            type="submit"
            disabled={busy}
            style={{
              flex: 1,
              padding: '0.5rem 1rem',
              fontSize: '0.85rem',
              borderRadius: 4,
              border: 'none',
              background: 'var(--color-accent, #2f81f7)',
              color: '#fff',
              cursor: busy ? 'progress' : 'pointer',
              opacity: busy ? 0.7 : 1,
            }}
          >
            {busy ? 'Connecting…' : 'Connect'}
          </button>
          <button
            type="button"
            onClick={cancelSignIn}
            disabled={busy}
            style={{
              padding: '0.5rem 1rem',
              fontSize: '0.85rem',
              borderRadius: 4,
              border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
              background: 'transparent',
              color: 'inherit',
              cursor: 'pointer',
            }}
          >
            Cancel
          </button>
        </div>
      </form>
    </div>
  )
}

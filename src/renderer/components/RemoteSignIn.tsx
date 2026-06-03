// RemoteSignIn — the full-screen sign-in for a saved K2 server with no
// live session token (or a rejected/expired one). connect-users (#617):
//
//   "Password not remembered (or remembered-but-rejected/expired) →
//    full-screen sign-in for that specific server (pre-filled label +
//    address + username, focus the password field), then connect."
//
// Mounted by ConnectionGate when `pendingSignIn` is set on the
// connect-host store. On success it:
//   - exchanges {username, password} for a session token via
//     loginToHost (POST /cli/auth/login) — which commits the session
//     token into the store + caches it to the keychain
//   - if "Remember password" → caches the PASSWORD to the OS keychain
//     (rememberPassword) and persists remember:true; else forgets it
//   - selectHost(host) to commit the switch (gate re-polls + mounts)
//
// If the password is already remembered, we auto-login on mount without
// prompting.

import React, { useEffect, useRef, useState } from 'react'
import {
  useConnectHostStore,
  rememberPassword,
  resolvePassword,
  forgetPassword,
  loginToHost,
  type ConnectHost,
} from '@/stores/connect-host'

export function RemoteSignIn({ host }: { host: ConnectHost }): React.JSX.Element {
  const selectHost = useConnectHostStore((s) => s.selectHost)
  const addHost = useConnectHostStore((s) => s.addHost)
  const cancelSignIn = useConnectHostStore((s) => s.cancelSignIn)

  const [password, setPassword] = useState('')
  // Default the remember toggle to the host's saved intent.
  const [remember, setRemember] = useState(host.remember)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const passwordRef = useRef<HTMLInputElement | null>(null)

  const address = host.secure && host.port === 443
    ? host.hostname
    : `${host.hostname}:${host.port}`

  // Shared login path used by both the auto-login effect and the submit
  // handler. On success commits + switches; on failure surfaces the
  // reason and (for auto-login) leaves the form for manual entry.
  const doLogin = async (pw: string, rememberPw: boolean): Promise<boolean> => {
    const result = await loginToHost(host, pw)
    if (!result.ok) {
      setError(result.reason)
      return false
    }
    // loginToHost committed the session token + lastConnectedAt into the
    // store. Persist remember intent + the keychain side.
    const refreshed = useConnectHostStore.getState().hosts.find((h) => h.id === host.id)
    const updated: ConnectHost = {
      ...(refreshed ?? host),
      token: result.token,
      remember: rememberPw,
      lastConnectedAt: Date.now(),
    }
    addHost(updated) // re-persists the (token-less) list with remember + lastConnectedAt
    if (rememberPw) {
      await rememberPassword(host.id, pw)
    } else {
      await forgetPassword(host.id)
    }
    // Switch — the gate re-polls against the new host and mounts on accept.
    selectHost(updated)
    return true
  }

  // Auto-login if the password was remembered; otherwise focus the field
  // for manual entry. (connect-users #617: "If the password was
  // remembered, auto-login without prompting.")
  useEffect(() => {
    let cancelled = false
    void (async () => {
      if (host.remember) {
        const pw = await resolvePassword(host.id)
        if (cancelled) return
        if (pw) {
          setBusy(true)
          const ok = await doLogin(pw, true)
          if (cancelled) return
          if (ok) return // switched away — overlay will unmount
          // Remembered password was rejected/expired → fall through to
          // manual entry.
          setBusy(false)
        }
      }
      passwordRef.current?.focus()
    })()
    return () => { cancelled = true }
    // host id is the stable identity; doLogin closes over `host` which is
    // stable for a given pendingSignIn render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [host.id])

  const submit = async (): Promise<void> => {
    if (!password) {
      setError('Enter the server password.')
      return
    }
    setBusy(true)
    setError(null)
    const ok = await doLogin(password, remember)
    if (!ok) setBusy(false)
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

        {host.username && (
          <label style={{ display: 'flex', flexDirection: 'column', gap: 5, fontSize: '0.78rem', opacity: 0.85 }}>
            Username
            <input
              type="text"
              value={host.username}
              readOnly
              autoComplete="username"
              style={{
                padding: '0.5rem 0.65rem',
                fontSize: '0.85rem',
                borderRadius: 4,
                border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
                background: 'var(--color-bg, rgba(0,0,0,0.3))',
                color: 'inherit',
                outline: 'none',
                opacity: 0.7,
              }}
            />
          </label>
        )}

        <label style={{ display: 'flex', flexDirection: 'column', gap: 5, fontSize: '0.78rem', opacity: 0.85 }}>
          Password
          <input
            ref={passwordRef}
            type="password"
            value={password}
            disabled={busy}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="Server password"
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

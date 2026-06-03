// Settings → K2 Connect — the HOST / expose side (PRD §6, build order
// MVP). The OTHER direction from Connections (§1–5, the client connecting
// OUT): "K2 Connect" is where THIS device exposes its OWN daemon at a
// subdomain it owns under k2.dev via the frpc reverse tunnel.
//
// MVP (buildable now, no control-plane changes):
//   - tunnel config form: token + subdomain + serverAddr (default
//     178.156.232.105:7000) → Save (POST /cli/tunnel/config; the token is
//     redacted on read → tokenSet:bool, never echoed back)
//   - Start / Stop + live status (GET /cli/tunnel/status) + public URL
//     https://<sub>.k2.dev + a "frpc not installed" hint surfaced from the
//     connector's error.
//
// ACCOUNT FLOW (live): k2.dev account sign-in (Supabase Auth, email +
//   password) → list the subdomains the user owns → pick one → it auto-
//   fills + saves the tunnel config to the daemon. The refresh token is
//   persisted to the OS keychain (account service, distinct from the host-
//   token service); the access token stays in memory. See lib/k2-account.ts.
//   Per PRD §6.2 the subdomains are the $2.99/mo purchased ones. The manual
//   token + subdomain form remains behind an "Advanced / manual" toggle as
//   a fallback.
//
// OUT OF SCOPE here: the daemon "Users / Access" multi-user feature
// (task #617) — that's the daemon-owned access list (PRD §6.2), a
// separate area from this account/expose page.

import React, { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'
import { SettingRow, SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import {
  signIn as accountSignIn,
  signOut as accountSignOut,
  refreshSession,
  listSubdomains,
  type K2Session,
  type K2Subdomain,
} from '../lib/k2-account'

const DEFAULT_SERVER_ADDR = '178.156.232.105'
const DEFAULT_SERVER_PORT = 7000

// Keychain coordinates for the k2.dev ACCOUNT session. Kept distinct from
// the host-token service (`com.k2so.connect.host-token`) so the two never
// collide. Only the refresh token + email are persisted; the access token
// is memory-only.
const ACCOUNT_KEYCHAIN_SERVICE = 'com.k2so.connect.account'
const SESSION_ACCOUNT_KEY = 'session-refresh-token'
const EMAIL_ACCOUNT_KEY = 'session-email'

export const K2_CONNECT_MANIFEST: SettingEntry[] = [
  { id: 'k2-connect.account-login', section: 'k2-connect', label: 'Sign in to K2 Connect', description: 'Sign in to k2.dev to pick a purchased subdomain', keywords: ['login', 'sign in', 'account', 'k2.dev', 'email', 'password'] },
  { id: 'k2-connect.subdomain-list', section: 'k2-connect', label: 'Your subdomains', description: 'Purchased k2.dev subdomains bound to this device', keywords: ['subdomain', 'purchased', 'list', 'bind', 'select'] },
  { id: 'k2-connect.subdomain', section: 'k2-connect', label: 'Subdomain', description: 'The k2.dev subdomain this device exposes as', keywords: ['subdomain', 'expose', 'tunnel', 'k2.dev', 'public url', 'host'] },
  { id: 'k2-connect.token', section: 'k2-connect', label: 'K2 Connect Token', description: 'Bearer token for the frpc tunnel', keywords: ['token', 'tunnel', 'frpc', 'auth', 'bearer'] },
  { id: 'k2-connect.server-addr', section: 'k2-connect', label: 'Tunnel Server Address', description: 'The K2 Connect frps endpoint', keywords: ['server', 'frps', 'address', 'hetzner', 'endpoint'] },
  { id: 'k2-connect.start-stop', section: 'k2-connect', label: 'Start / Stop Tunnel', description: 'Expose this device at a public URL', keywords: ['start', 'stop', 'tunnel', 'expose', 'connect'] },
]

interface TunnelStatus {
  running: boolean
  public_url: string | null
  frpc_installed: boolean
}

interface TunnelConfigView {
  serverAddr: string
  serverPort: number
  subdomain: string
  tokenSet: boolean
  publicUrl: string | null
}

async function tunnelGet(suffix: string): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(`${daemonHttpBase(creds)}/cli/tunnel/${suffix}?token=${creds.token}`, { method: 'GET' })
}

async function tunnelPost(suffix: string, body?: unknown): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(`${daemonHttpBase(creds)}/cli/tunnel/${suffix}?token=${creds.token}`, {
    method: 'POST',
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

async function errText(res: Response): Promise<string> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text)
    if (parsed && typeof parsed.error === 'string') return parsed.error
  } catch {
    /* raw text */
  }
  return text || `request failed (${res.status})`
}

// ── Account keychain helpers (refresh token + email only) ───────────────
// Best-effort + Tauri-only; they no-op (resolve) in the vitest/web env.

async function saveAccountSession(refreshToken: string, email: string): Promise<void> {
  try {
    await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY, secret: refreshToken })
    await invoke('k2_secret_set', { service: ACCOUNT_KEYCHAIN_SERVICE, account: EMAIL_ACCOUNT_KEY, secret: email })
  } catch {
    /* keychain unavailable — session stays in memory for this run */
  }
}

async function readAccountRefreshToken(): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY })
    return secret ?? null
  } catch {
    return null
  }
}

async function clearAccountSession(): Promise<void> {
  try {
    await invoke('k2_secret_delete', { service: ACCOUNT_KEYCHAIN_SERVICE, account: SESSION_ACCOUNT_KEY })
  } catch { /* idempotent */ }
  try {
    await invoke('k2_secret_delete', { service: ACCOUNT_KEYCHAIN_SERVICE, account: EMAIL_ACCOUNT_KEY })
  } catch { /* idempotent */ }
}

export function K2ConnectSection(): React.JSX.Element {
  const [serverAddr, setServerAddr] = useState(DEFAULT_SERVER_ADDR)
  const [serverPort, setServerPort] = useState(String(DEFAULT_SERVER_PORT))
  const [subdomain, setSubdomain] = useState('')
  const [token, setToken] = useState('')
  const [tokenSet, setTokenSet] = useState(false)
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [savedMsg, setSavedMsg] = useState<string | null>(null)

  // ── Account / subdomain-picker state ──────────────────────────────────
  const [session, setSession] = useState<K2Session | null>(null)
  const [subdomains, setSubdomains] = useState<K2Subdomain[]>([])
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [authBusy, setAuthBusy] = useState(false)
  const [authError, setAuthError] = useState<string | null>(null)
  const [selectedLabel, setSelectedLabel] = useState<string | null>(null)
  const [boundMsg, setBoundMsg] = useState<string | null>(null)
  const [showAdvanced, setShowAdvanced] = useState(false)

  // Load config + status on mount, then poll status while mounted.
  useEffect(() => {
    void (async () => {
      try {
        const res = await tunnelGet('config')
        if (res.ok) {
          const cfg = (await res.json()) as TunnelConfigView
          setServerAddr(cfg.serverAddr || DEFAULT_SERVER_ADDR)
          setServerPort(String(cfg.serverPort || DEFAULT_SERVER_PORT))
          setSubdomain(cfg.subdomain || '')
          setTokenSet(cfg.tokenSet)
        }
      } catch { /* ignore */ }
      void refreshStatus()
    })()
    // Restore the account session from the keychain (independent of the
    // tunnel-config load above — must NOT block it).
    void (async () => {
      const stored = await readAccountRefreshToken()
      if (!stored) return
      try {
        const fresh = await refreshSession(stored)
        setSession(fresh)
        // The refresh token may have rotated — persist the new one.
        await saveAccountSession(fresh.refreshToken, fresh.email)
        const subs = await listSubdomains(fresh.accessToken)
        setSubdomains(subs)
      } catch {
        // Expired / revoked — drop the stale credentials, stay logged out.
        await clearAccountSession()
        setSession(null)
      }
    })()
    const interval = setInterval(() => void refreshStatus(), 5000)
    return () => clearInterval(interval)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const refreshStatus = async (): Promise<void> => {
    try {
      const res = await tunnelGet('status')
      if (res.ok) setStatus((await res.json()) as TunnelStatus)
    } catch { /* ignore */ }
  }

  const saveConfig = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    setSavedMsg(null)
    const portNum = Number(serverPort)
    if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
      setError('Server port must be 1–65535')
      setBusy(false)
      return
    }
    // Only send the token when the user typed one (empty string would
    // CLEAR it server-side; undefined leaves the saved token untouched).
    const body: Record<string, unknown> = {
      serverAddr: serverAddr.trim(),
      serverPort: portNum,
      subdomain: subdomain.trim(),
    }
    if (token.trim()) body.token = token.trim()
    try {
      const res = await tunnelPost('config', body)
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // never keep the secret in the field after save
      setSavedMsg('Saved')
      setTimeout(() => setSavedMsg(null), 1500)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Save failed')
    } finally {
      setBusy(false)
    }
  }

  const startTunnel = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const sub = subdomain.trim()
      const res = await tunnelPost(`start${sub ? `?subdomain=${encodeURIComponent(sub)}` : ''}`)
      if (!res.ok) {
        // The connector surfaces the frpc-not-installed hint verbatim here.
        setError(await errText(res))
        return
      }
      await refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Start failed')
    } finally {
      setBusy(false)
    }
  }

  const stopTunnel = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const res = await tunnelPost('stop')
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      await refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Stop failed')
    } finally {
      setBusy(false)
    }
  }

  const handleSignIn = async (): Promise<void> => {
    setAuthBusy(true)
    setAuthError(null)
    try {
      const sess = await accountSignIn(email.trim(), password)
      setSession(sess)
      setPassword('')
      await saveAccountSession(sess.refreshToken, sess.email)
      try {
        const subs = await listSubdomains(sess.accessToken)
        setSubdomains(subs)
      } catch (e) {
        setAuthError(e instanceof Error ? e.message : 'Failed to load subdomains')
      }
    } catch (e) {
      setAuthError(e instanceof Error ? e.message : 'Sign in failed')
    } finally {
      setAuthBusy(false)
    }
  }

  const handleSignOut = async (): Promise<void> => {
    const token = session?.accessToken
    setSession(null)
    setSubdomains([])
    setSelectedLabel(null)
    setBoundMsg(null)
    setAuthError(null)
    await clearAccountSession()
    if (token) void accountSignOut(token)
  }

  // Bind a chosen subdomain: fill the manual-config state AND persist the
  // chosen subdomain + token straight to the daemon so tunnel.json is
  // updated immediately. Uses the row's values directly (state setters are
  // async, so we can't read `subdomain`/`token` back in this tick).
  const bindSubdomain = async (sub: K2Subdomain): Promise<void> => {
    setSelectedLabel(sub.label)
    setBoundMsg(null)
    setSubdomain(sub.label)
    setToken(sub.tunnel_token)
    setBusy(true)
    setError(null)
    try {
      const res = await tunnelPost('config', {
        serverAddr: serverAddr.trim() || DEFAULT_SERVER_ADDR,
        serverPort: Number(serverPort) || DEFAULT_SERVER_PORT,
        subdomain: sub.label,
        token: sub.tunnel_token,
      })
      if (!res.ok) {
        setError(await errText(res))
        return
      }
      const cfg = (await res.json()) as TunnelConfigView
      setTokenSet(cfg.tokenSet)
      setToken('') // don't keep the secret in the manual field after save
      setBoundMsg(`Bound ${sub.label}.k2.dev — Start the tunnel below.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to bind subdomain')
    } finally {
      setBusy(false)
    }
  }

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? (subdomain.trim() ? `https://${subdomain.trim()}.k2.dev` : null)
  const frpcMissing = status ? !status.frpc_installed : false

  // The custom dropdown's option values are the subdomain labels; map a
  // chosen value back to its full row so bindSubdomain gets the token.
  const subdomainOptions = subdomains.map((s) => ({
    value: s.label,
    label: s.status === 'active' ? `${s.label}.k2.dev` : `${s.label}.k2.dev (${s.status})`,
  }))
  const handleSubdomainPick = async (value: string): Promise<void> => {
    const row = subdomains.find((s) => s.label === value)
    if (row) await bindSubdomain(row)
  }

  return (
    <div className="w-full">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        K2 Connect
        <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        Expose THIS device&apos;s K2 daemon at a public URL so you can reach it from another
        computer (the server switcher) or your phone (K2 Companion). One tunnel, both clients.
      </p>

      <div className="space-y-5">
        {/* ── Account: sign-in + purchased-subdomain picker ──────────── */}
        <SettingsGroup
          title="Account"
          badge={
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              beta
            </span>
          }
        >
          {!session ? (
            <div data-settings-id="k2-connect.account-login">
              <div className="flex flex-col mb-2">
                <span className="text-xs text-[var(--color-text-secondary)]">Sign in to K2 Connect</span>
                <span className="text-[10px] text-[var(--color-text-muted)]">
                  Sign in with your k2.dev account to pick a purchased subdomain automatically.
                </span>
              </div>
              <form
                className="space-y-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  void handleSignIn()
                }}
              >
                <input
                  className={inputCls}
                  type="email"
                  autoComplete="email"
                  placeholder="you@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
                <input
                  className={inputCls}
                  type="password"
                  autoComplete="current-password"
                  placeholder="Password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
                {authError && (
                  <div className="text-[10px] text-red-400 px-2 py-1 border border-red-400/20 bg-red-400/5">{authError}</div>
                )}
                <button
                  type="submit"
                  disabled={authBusy || !email.trim() || !password}
                  className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
                >
                  {authBusy ? 'Signing in…' : 'Sign in'}
                </button>
              </form>
            </div>
          ) : (
            <>
              <SettingRow
                settingId="k2-connect.account-login"
                label={
                  <>
                    Signed in as{' '}
                    <span className="text-[var(--color-text-primary)]">{session.email || 'your account'}</span>
                  </>
                }
              >
                <button
                  onClick={() => void handleSignOut()}
                  className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:underline no-drag cursor-pointer"
                >
                  Sign out
                </button>
              </SettingRow>

              <SettingRow settingId="k2-connect.subdomain-list" label="Your subdomains">
                {subdomains.length === 0 ? (
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    No subdomains yet — buy one at{' '}
                    <a
                      href="https://k2.dev"
                      target="_blank"
                      rel="noreferrer"
                      className="text-[var(--color-accent)] hover:underline"
                    >
                      k2.dev
                    </a>
                    .
                  </span>
                ) : (
                  <SettingDropdown
                    value={selectedLabel ?? subdomain ?? ''}
                    options={subdomainOptions}
                    onChange={(v) => void handleSubdomainPick(v)}
                  />
                )}
              </SettingRow>
              {boundMsg && (
                <div className="text-[10px] text-green-400 py-1">{boundMsg}</div>
              )}
            </>
          )}
        </SettingsGroup>

        {/* ── Live status ──────────────────────────────────────────── */}
        <div className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]">
          <span
            className="w-2 h-2 flex-shrink-0 rounded-full"
            style={{ backgroundColor: running ? '#22c55e' : '#6b7280' }}
          />
          <span className="text-xs text-[var(--color-text-secondary)]">
            {running ? 'Tunnel running' : 'Tunnel stopped'}
          </span>
          {running && publicUrl && (
            <a
              href={publicUrl}
              target="_blank"
              rel="noreferrer"
              className="ml-auto text-[10px] text-[var(--color-accent)] hover:underline font-mono truncate max-w-[220px]"
            >
              {publicUrl}
            </a>
          )}
        </div>

        {frpcMissing && (
          <div className="flex items-start gap-2 px-3 py-2 border border-amber-400/30 bg-amber-400/5">
            <span className="text-amber-400 text-sm leading-none flex-shrink-0 mt-0.5">&#9888;</span>
            <div className="text-[10px] text-amber-300/80 leading-relaxed">
              <strong className="text-amber-300">frpc not installed.</strong>{' '}
              K2 Connect needs the <span className="font-mono">frpc</span> client on your PATH.
              Install it from <span className="font-mono">github.com/fatedier/frp/releases</span>{' '}
              (or via <span className="font-mono">brew install frpc</span>) and try again.
            </div>
          </div>
        )}

        {error && <div className="text-[10px] text-red-400 px-3 py-1.5 border border-red-400/20 bg-red-400/5">{error}</div>}

        {/* ── Advanced / manual config (fallback) ──────────────────── */}
        <div>
          <button
            onClick={() => setShowAdvanced((v) => !v)}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] no-drag cursor-pointer mb-2"
          >
            {showAdvanced ? '▾' : '▸'} Advanced / manual config
          </button>
          {showAdvanced && (
            <SettingsGroup title="Advanced / manual config">
              <SettingRow settingId="k2-connect.subdomain" label="Subdomain">
                <input
                  className={inputCls}
                  style={{ maxWidth: 200 }}
                  placeholder="e.g. alice"
                  value={subdomain}
                  onChange={(e) => setSubdomain(e.target.value)}
                />
              </SettingRow>
              <SettingRow
                settingId="k2-connect.token"
                label={
                  <>
                    Token
                    {tokenSet && <span className="ml-2 text-[10px] text-green-400">Set</span>}
                  </>
                }
              >
                <input
                  className={inputCls}
                  style={{ maxWidth: 200 }}
                  type="password"
                  placeholder={tokenSet ? '•••••••• (leave blank to keep)' : 'K2 Connect token'}
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
              </SettingRow>
              <SettingRow settingId="k2-connect.server-addr" label="Tunnel Server">
                <div className="flex gap-1.5">
                  <input
                    className={inputCls}
                    style={{ maxWidth: 150 }}
                    placeholder="server address"
                    value={serverAddr}
                    onChange={(e) => setServerAddr(e.target.value)}
                  />
                  <input
                    className={inputCls}
                    style={{ maxWidth: 64 }}
                    placeholder="port"
                    value={serverPort}
                    onChange={(e) => setServerPort(e.target.value)}
                  />
                </div>
              </SettingRow>
              <div className="flex items-center gap-2 pt-2">
                <button
                  onClick={() => void saveConfig()}
                  disabled={busy}
                  className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
                >
                  {busy ? 'Saving…' : 'Save'}
                </button>
                {savedMsg && <span className="text-[10px] text-green-400">{savedMsg}</span>}
              </div>
            </SettingsGroup>
          )}
        </div>

        <div className="flex items-center gap-2" data-settings-id="k2-connect.start-stop">
          <div className="ml-auto flex items-center gap-2">
            {running ? (
              <button
                onClick={() => void stopTunnel()}
                disabled={busy}
                className="px-3 py-1 text-[11px] text-white bg-red-500/80 hover:bg-red-500 no-drag cursor-pointer disabled:opacity-60"
              >
                Stop tunnel
              </button>
            ) : (
              <button
                onClick={() => void startTunnel()}
                disabled={busy}
                className="px-3 py-1 text-[11px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
              >
                Start tunnel
              </button>
            )}
          </div>
        </div>

        <div className="text-[10px] text-[var(--color-text-muted)] space-y-1">
          <p>1. Sign in to k2.dev above and pick a subdomain you own (e.g. <span className="font-mono">alice</span>).</p>
          <p>2. Start the tunnel — your daemon becomes reachable at <span className="font-mono">https://&lt;sub&gt;.k2.dev</span>.</p>
          <p>3. Add that address as a server on another computer (Settings → Connections), or pair the K2 Companion app.</p>
        </div>
      </div>
    </div>
  )
}

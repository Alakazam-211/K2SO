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
// STUBS (depend on the proprietary control-plane account API — NOT built):
//   - disabled "Log in to K2 Connect (coming soon)"
//   - purchased-subdomain picker placeholder
//   See TODO(control-plane-account-api). Per PRD §6.2 the real flow is
//   account-login → list purchased subdomains ($2.99/mo each) → pick which
//   binds this device. The manual token + subdomain form stands in until
//   that API lands.
//
// OUT OF SCOPE here: the daemon "Users / Access" multi-user feature
// (task #617) — that's the daemon-owned access list (PRD §6.2), a
// separate area from this account/expose page.

import React, { useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { SettingEntry } from '../searchManifest'

const DEFAULT_SERVER_ADDR = '178.156.232.105'
const DEFAULT_SERVER_PORT = 7000

export const K2_CONNECT_MANIFEST: SettingEntry[] = [
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

  const inputCls =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? (subdomain.trim() ? `https://${subdomain.trim()}.k2.dev` : null)
  const frpcMissing = status ? !status.frpc_installed : false

  return (
    <div className="max-w-xl">
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

      {/* ── Account login + subdomain picker (STUB) ──────────────────── */}
      {/* TODO(control-plane-account-api): the real flow is k2.dev account
          login → list purchased subdomains ($2.99/mo each, PRD §6.2) →
          pick which binds this device. Until the proprietary control-plane
          /account + /subdomains API exists, the manual token + subdomain
          form below stands in. */}
      <div className="mb-4 px-3 py-3 border border-dashed border-[var(--color-border)] opacity-80">
        <div className="flex items-center justify-between">
          <div className="flex flex-col">
            <span className="text-xs text-[var(--color-text-secondary)]">Log in to K2 Connect</span>
            <span className="text-[10px] text-[var(--color-text-muted)]">
              Sign in to k2.dev to pick a purchased subdomain automatically.
            </span>
          </div>
          <button
            disabled
            title="Coming soon — requires the K2 Connect account API"
            className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-muted)] cursor-not-allowed"
          >
            Log in (coming soon)
          </button>
        </div>
        <div className="mt-2">
          <label className="text-[10px] text-[var(--color-text-muted)]">Purchased subdomain</label>
          <select
            disabled
            title="Coming soon — populated after K2 Connect account login"
            className="w-full mt-1 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-muted)] cursor-not-allowed"
          >
            <option>Sign in to choose a subdomain…</option>
          </select>
        </div>
      </div>

      {/* ── Live status ──────────────────────────────────────────────── */}
      <div className="flex items-center gap-2 mb-4 px-3 py-2 border border-[var(--color-border)]">
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
        <div className="flex items-start gap-2 mb-4 px-3 py-2 border border-amber-400/30 bg-amber-400/5">
          <span className="text-amber-400 text-sm leading-none flex-shrink-0 mt-0.5">&#9888;</span>
          <div className="text-[10px] text-amber-300/80 leading-relaxed">
            <strong className="text-amber-300">frpc not installed.</strong>{' '}
            K2 Connect needs the <span className="font-mono">frpc</span> client on your PATH.
            Install it from <span className="font-mono">github.com/fatedier/frp/releases</span>{' '}
            (or via <span className="font-mono">brew install frpc</span>) and try again.
          </div>
        </div>
      )}

      {error && <div className="text-[10px] text-red-400 mb-3 px-3 py-1.5 border border-red-400/20 bg-red-400/5">{error}</div>}

      {/* ── Manual tunnel config (MVP) ───────────────────────────────── */}
      <div className="space-y-0">
        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]" data-settings-id="k2-connect.subdomain">Subdomain</span>
          <input
            className={inputCls}
            style={{ maxWidth: 200 }}
            placeholder="e.g. rosson"
            value={subdomain}
            onChange={(e) => setSubdomain(e.target.value)}
          />
        </div>
        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]" data-settings-id="k2-connect.token">Token</span>
            {tokenSet && <span className="ml-2 text-[10px] text-green-400">Set</span>}
          </div>
          <input
            className={inputCls}
            style={{ maxWidth: 200 }}
            type="password"
            placeholder={tokenSet ? '•••••••• (leave blank to keep)' : 'K2 Connect token'}
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
        </div>
        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]" data-settings-id="k2-connect.server-addr">Tunnel Server</span>
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
        </div>
      </div>

      <div className="flex items-center gap-2 mt-4" data-settings-id="k2-connect.start-stop">
        <button
          onClick={() => void saveConfig()}
          disabled={busy}
          className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60"
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
        {savedMsg && <span className="text-[10px] text-green-400">{savedMsg}</span>}
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

      <div className="mt-6 text-[10px] text-[var(--color-text-muted)] space-y-1">
        <p>1. Enter your K2 Connect token + the subdomain you own (e.g. <span className="font-mono">rosson</span>).</p>
        <p>2. Save, then Start the tunnel — your daemon becomes reachable at <span className="font-mono">https://&lt;sub&gt;.k2.dev</span>.</p>
        <p>3. Add that address as a server on another computer (Settings → Connections), or pair the K2 Companion app.</p>
      </div>
    </div>
  )
}

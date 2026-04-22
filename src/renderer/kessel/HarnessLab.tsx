// Harness Lab v1 — visual Kessel validation surface.
//
// Self-contained React pane: pick a command, spawn a session via
// the daemon's /cli/sessions/spawn, render it with SessionStreamView.
// Not integrated with tabs/projects/settings — intentional. Gives
// us a screenshot gate for Phase 4.5 before the deeper flow work
// (I8 mouse selection, I9 full spawn-path integration).
//
// Future iterations (documented in the Phase 4.5 plan's Q2 answer):
//   - Device-size preset buttons (iPhone, iPad, laptop, desktop)
//     that snap the container to those dims and re-flow via our
//     ResizeObserver.
//   - Side-by-side harness config editor with live reload.
//   - Saved presets per harness.
//
// Today: a dropdown + Spawn button + the Kessel pane. Enough for
// "does a real TUI render through the new pipeline."

import React, { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { SessionStreamView } from './SessionStreamView'

interface DaemonWsUrl {
  state: 'available' | 'not_installed'
  port?: number
  token?: string
  reason?: string
}

interface SpawnResult {
  sessionId: string
  agentName: string
}

const PRESETS: Array<{ label: string; command: string }> = [
  { label: 'bash (no profile)', command: 'bash --noprofile --norc' },
  { label: 'zsh (no rc)', command: 'zsh -f' },
  { label: 'claude (interactive)', command: 'claude' },
  { label: 'claude --help', command: 'claude --help' },
  { label: 'htop', command: 'htop' },
  { label: 'vim', command: 'vim' },
  { label: 'echo loop', command: 'for i in 1 2 3 4 5; do echo "kessel-$i"; sleep 0.3; done' },
]

export interface HarnessLabProps {
  open: boolean
  onClose: () => void
}

export function HarnessLab({ open, onClose }: HarnessLabProps): React.JSX.Element | null {
  const [wsUrl, setWsUrl] = useState<DaemonWsUrl | null>(null)
  const [command, setCommand] = useState(PRESETS[0].command)
  const [session, setSession] = useState<SpawnResult | null>(null)
  const [status, setStatus] = useState('')

  // Fetch daemon port + token on mount.
  useEffect(() => {
    if (!open) return
    invoke<DaemonWsUrl>('daemon_ws_url')
      .then((r) => setWsUrl(r))
      .catch((e) => setStatus(`daemon_ws_url failed: ${e}`))
  }, [open])

  // Close on Escape.
  useEffect(() => {
    if (!open) return
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [open, onClose])

  async function spawn(): Promise<void> {
    if (!wsUrl || wsUrl.state !== 'available' || !wsUrl.port || !wsUrl.token) {
      setStatus('daemon not reachable — is it running?')
      return
    }
    setStatus('spawning...')
    try {
      const res = await fetch(
        `http://127.0.0.1:${wsUrl.port}/cli/sessions/spawn?token=${wsUrl.token}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            agent_name: `harness-lab-${Date.now()}`,
            cwd: '/tmp',
            command,
            cols: 80,
            rows: 24,
          }),
        },
      )
      if (!res.ok) {
        const body = await res.text()
        setStatus(`spawn failed: HTTP ${res.status} ${body}`)
        return
      }
      const data = (await res.json()) as SpawnResult
      setSession(data)
      setStatus(`session ${data.sessionId.slice(0, 8)}… running`)
    } catch (e) {
      setStatus(`spawn error: ${String(e)}`)
    }
  }

  if (!open) return null

  const overlay: React.CSSProperties = {
    position: 'fixed',
    top: 0,
    left: 0,
    right: 0,
    bottom: 0,
    background: 'rgba(0,0,0,0.7)',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    zIndex: 10000,
  }
  const panel: React.CSSProperties = {
    background: '#1a1a1a',
    border: '1px solid #3a3a3a',
    borderRadius: 8,
    padding: 16,
    minWidth: 720,
    maxWidth: '90vw',
    maxHeight: '90vh',
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    color: '#e0e0e0',
    fontFamily: 'system-ui, -apple-system, sans-serif',
  }
  const toolbar: React.CSSProperties = {
    display: 'flex',
    gap: 8,
    alignItems: 'center',
  }
  const select: React.CSSProperties = {
    background: '#222',
    color: '#e0e0e0',
    border: '1px solid #3a3a3a',
    borderRadius: 4,
    padding: '4px 8px',
    fontSize: 13,
    minWidth: 260,
  }
  const input: React.CSSProperties = { ...select, minWidth: 360, flex: 1 }
  const btn: React.CSSProperties = {
    background: '#2a2a6a',
    color: '#fff',
    border: 'none',
    borderRadius: 4,
    padding: '6px 14px',
    cursor: 'pointer',
    fontSize: 13,
  }
  const closeBtn: React.CSSProperties = { ...btn, background: '#555', marginLeft: 'auto' }

  return (
    <div style={overlay} onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div style={panel}>
        <div style={toolbar}>
          <strong>Kessel · Harness Lab</strong>
          <span style={{ color: '#888', fontSize: 12 }}>{status}</span>
          <button style={closeBtn} onClick={onClose}>
            Close (Esc)
          </button>
        </div>

        <div style={toolbar}>
          <select
            style={select}
            value={command}
            onChange={(e) => setCommand(e.target.value)}
          >
            {PRESETS.map((p) => (
              <option key={p.command} value={p.command}>
                {p.label}
              </option>
            ))}
          </select>
          <input
            style={input}
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder="shell command to spawn via daemon"
          />
          <button style={btn} onClick={spawn}>
            Spawn
          </button>
        </div>

        <div
          style={{
            flex: 1,
            // Explicit height (not just min-height) so SessionStreamView's
            // `height: 100%` resolves to a real pixel value. Flex-row
            // parent + `height: 100%` child needs an ancestor with
            // a computed height; min-height doesn't count for %
            // resolution in CSS spec.
            height: 560,
            display: 'flex',
            border: '1px solid #3a3a3a',
            borderRadius: 4,
            overflow: 'hidden',
          }}
        >
          {session && wsUrl?.state === 'available' && wsUrl.port && wsUrl.token ? (
            <SessionStreamView
              sessionId={session.sessionId}
              port={wsUrl.port}
              token={wsUrl.token}
              cols={80}
              rows={24}
              autoResize
              interactive
              onReady={(n) => setStatus(`ready (${n} replay frames)`)}
              onError={(msg) => setStatus(`error: ${msg}`)}
            />
          ) : (
            <div style={{ margin: 'auto', color: '#888' }}>
              Pick a command and press Spawn.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

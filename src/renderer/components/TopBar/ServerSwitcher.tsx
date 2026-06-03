// ServerSwitcher — the always-visible top-bar control that picks which
// K2 daemon the app talks to (K2 Connect client UX, build order step #2).
//
// Dropdown contents (PRD §1):
//   - "This Mac" (local bundled daemon) — always first, never needs auth.
//   - every saved ConnectHost.
//   - "Add a server…" — meant to route to Settings → Connections (step
//     #3). That page doesn't exist yet, so for now this opens a minimal
//     INTERIM inline form that calls addHost(), making the switcher
//     testable against a second local daemon. TODO(#3): replace with
//     Settings → Connections.
//
// The active host's label + a status dot (connected / connecting /
// offline) sit in the always-visible trigger. The color-coded latency
// readout is step #5 — a clean seam is left (the dot reads the gate's
// connectionStatus today).
//
// When `activeHost === 'local'` this is purely cosmetic — selecting
// "This Mac" is the no-op default and behaves byte-identically to today.

import { useState, useRef, useEffect, useCallback } from 'react'
import {
  useConnectHostStore,
  type ConnectHost,
  type ConnectionStatus,
} from '@/stores/connect-host'

function statusColor(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return '#3fb950' // green
    case 'connecting':
      return '#d29922' // amber
    case 'offline':
      return '#f85149' // red
  }
}

function StatusDot({ status }: { status: ConnectionStatus }): React.JSX.Element {
  return (
    <span
      aria-label={`connection ${status}`}
      title={`Connection: ${status}`}
      style={{
        width: 7,
        height: 7,
        borderRadius: '50%',
        background: statusColor(status),
        flexShrink: 0,
        display: 'inline-block',
      }}
    />
  )
}

export default function ServerSwitcher(): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const selectHost = useConnectHostStore((s) => s.selectHost)

  const [open, setOpen] = useState(false)
  const [showAddForm, setShowAddForm] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  // Close the dropdown on outside click / Escape.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false)
        setShowAddForm(false)
      }
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setOpen(false)
        setShowAddForm(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const activeLabel = activeHost === 'local' ? 'Local' : activeHost.label

  const pick = useCallback(
    (h: 'local' | ConnectHost) => {
      selectHost(h)
      setOpen(false)
      setShowAddForm(false)
    },
    [selectHost],
  )

  return (
    <div
      ref={rootRef}
      className="relative no-drag"
      style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
    >
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 h-6 px-2 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors rounded"
        title="Switch K2 server"
      >
        <StatusDot status={connectionStatus} />
        <span className="max-w-[140px] truncate">{activeLabel}</span>
        <svg width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 4 5 7 8 4" />
        </svg>
      </button>

      {open && (
        <div
          className="absolute left-0 top-7 z-50 min-w-[220px] rounded border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-lg py-1 text-[12px]"
        >
          {/* Local — always first */}
          <SwitcherRow
            label="Local"
            active={activeHost === 'local'}
            statusDot={activeHost === 'local' ? connectionStatus : null}
            onClick={() => pick('local')}
          />

          {hosts.length > 0 && <div className="my-1 h-px bg-[var(--color-border)]" />}

          {hosts.map((h) => {
            const isActive = activeHost !== 'local' && activeHost.id === h.id
            return (
              <SwitcherRow
                key={h.id}
                label={h.label}
                sublabel={`${h.hostname}:${h.port}`}
                active={isActive}
                statusDot={isActive ? connectionStatus : null}
                onClick={() => pick(h)}
              />
            )
          })}

          <div className="my-1 h-px bg-[var(--color-border)]" />

          {/* TODO(#3): replace with route to Settings → Connections */}
          <button
            onClick={() => setShowAddForm((v) => !v)}
            className="w-full text-left px-3 py-1.5 text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          >
            Add a server…
          </button>

          {showAddForm && (
            <AddServerForm
              onDone={() => {
                setShowAddForm(false)
                setOpen(false)
              }}
            />
          )}
        </div>
      )}
    </div>
  )
}

function SwitcherRow({
  label,
  sublabel,
  active,
  statusDot,
  onClick,
}: {
  label: string
  sublabel?: string
  active: boolean
  statusDot: ConnectionStatus | null
  onClick: () => void
}): React.JSX.Element {
  return (
    <button
      onClick={onClick}
      className={`w-full text-left px-3 py-1.5 flex items-center gap-2 transition-colors ${
        active
          ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
          : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
      }`}
    >
      {statusDot !== null ? (
        <StatusDot status={statusDot} />
      ) : (
        <span style={{ width: 7, flexShrink: 0 }} />
      )}
      <span className="flex flex-col min-w-0">
        <span className="truncate">{label}</span>
        {sublabel && (
          <span className="text-[10px] text-[var(--color-text-muted)] truncate">{sublabel}</span>
        )}
      </span>
      {active && (
        <svg className="ml-auto" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="2 6 5 9 10 3" />
        </svg>
      )}
    </button>
  )
}

// ── INTERIM add-server form ─────────────────────────────────────────────
// TODO(#3): replace with Settings → Connections. This minimal form exists
// only so the switcher is testable against a second local daemon before
// the real address-book UI lands. Token is held in-memory (no keychain
// yet — step #3).
function AddServerForm({ onDone }: { onDone: () => void }): React.JSX.Element {
  const addHost = useConnectHostStore((s) => s.addHost)
  const selectHost = useConnectHostStore((s) => s.selectHost)
  const [label, setLabel] = useState('')
  const [hostname, setHostname] = useState('127.0.0.1')
  const [port, setPort] = useState('')
  const [token, setToken] = useState('')
  const [remember, setRemember] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = (): void => {
    const portNum = Number(port)
    if (!label.trim()) {
      setError('Label is required')
      return
    }
    if (!hostname.trim()) {
      setError('Hostname is required')
      return
    }
    if (!Number.isInteger(portNum) || portNum <= 0 || portNum > 65535) {
      setError('Port must be 1–65535')
      return
    }
    const host: ConnectHost = {
      id: `host-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      label: label.trim(),
      hostname: hostname.trim(),
      port: portNum,
      token: token.trim(),
      remember,
      lastConnectedAt: null,
    }
    addHost(host)
    // Switch to it immediately so the gate re-points + reconnects.
    selectHost(host)
    onDone()
  }

  const inputCls =
    'w-full px-2 py-1 text-[11px] rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]'

  return (
    <div className="px-3 py-2 flex flex-col gap-1.5 border-t border-[var(--color-border)]">
      <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
        Add server (interim)
      </div>
      <input
        className={inputCls}
        placeholder="Label (e.g. Test daemon)"
        value={label}
        onChange={(e) => setLabel(e.target.value)}
      />
      <div className="flex gap-1.5">
        <input
          className={inputCls}
          placeholder="hostname"
          value={hostname}
          onChange={(e) => setHostname(e.target.value)}
        />
        <input
          className={inputCls}
          style={{ maxWidth: 70 }}
          placeholder="port"
          value={port}
          onChange={(e) => setPort(e.target.value)}
        />
      </div>
      <input
        className={inputCls}
        placeholder="token"
        value={token}
        onChange={(e) => setToken(e.target.value)}
      />
      <label className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
        <input
          type="checkbox"
          checked={remember}
          onChange={(e) => setRemember(e.target.checked)}
        />
        Remember password
      </label>
      {error && <div className="text-[10px] text-[#f85149]">{error}</div>}
      <div className="flex gap-1.5 mt-1">
        <button
          onClick={submit}
          className="flex-1 px-2 py-1 text-[11px] rounded bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity"
        >
          Add &amp; connect
        </button>
        <button
          onClick={onDone}
          className="px-2 py-1 text-[11px] rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

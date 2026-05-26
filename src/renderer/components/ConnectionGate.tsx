/**
 * ConnectionGate — wraps the React app and gates render on daemon
 * reachability.
 *
 * Why this exists (0.39.2):
 *
 * On auto-update, the OLD daemon process stays in memory until
 * launchctl kickstart cycles it. Meanwhile the new K2SO.app launches,
 * React mounts, and the renderer's initial fetches (settings,
 * projects, workspaces) race the daemon restart. If those fetches
 * hit the daemon mid-restart, they fail silently and React mounts
 * against empty/stale data — visually presents as a blank app until
 * the user right-clicks → Reload.
 *
 * The gate polls the daemon's unauthenticated /ping endpoint until
 * it responds. Until then, it shows a "Connecting…" overlay. Once
 * the daemon is reachable, it unmounts the overlay and renders the
 * actual app — every store's initial fetch then runs against a
 * healthy daemon, no race.
 *
 * Designed to be reusable for K2 Connect: a remote daemon (Machine
 * A's daemon, accessed from Machine B over a tunnel) may be
 * transiently unreachable for the same reasons — restart, network
 * blip, tunnel reconnect. The gate's "retry until reachable, show
 * Connecting… while we wait" pattern is the same primitive.
 *
 * Happy path: /ping succeeds on first try, overlay never paints
 * (or paints for <100ms). User sees the app mount normally.
 *
 * Auto-update path: /ping fails for ~1-3s while daemon restarts,
 * gate shows the overlay, mounts the app once /ping responds. No
 * blank screen, no manual reload required.
 *
 * Permanent-failure path: after ~30s of failed polls, a Reload
 * button appears so the user can recover instead of staring at an
 * infinite spinner.
 */
import React, { useEffect, useState } from 'react'
import { getDaemonWs, invalidateDaemonWs } from '@/kessel/daemon-ws'

/** Hit the daemon's /ping endpoint with a short per-attempt timeout.
 *  Returns true if the daemon responded 2xx, false on any error. */
async function pingDaemon(): Promise<boolean> {
  try {
    const { port } = await getDaemonWs()
    const resp = await fetch(`http://127.0.0.1:${port}/ping`, {
      signal: AbortSignal.timeout(2000),
    })
    return resp.ok
  } catch {
    // Network error, timeout, port file missing, etc. — daemon
    // isn't ready yet. Invalidate cached port so the next poll
    // re-reads ~/.k2so/daemon.port (covers the case where kickstart
    // assigned a new port).
    invalidateDaemonWs()
    return false
  }
}

interface ConnectionGateProps {
  children: React.ReactNode
}

export function ConnectionGate({ children }: ConnectionGateProps): React.ReactElement {
  const [connected, setConnected] = useState(false)
  const [attempts, setAttempts] = useState(0)

  useEffect(() => {
    let cancelled = false
    let timeoutId: ReturnType<typeof setTimeout> | null = null

    const tryConnect = async (): Promise<void> => {
      const ok = await pingDaemon()
      if (cancelled) return
      if (ok) {
        setConnected(true)
        return
      }
      setAttempts((a) => a + 1)
      // 500ms backoff between attempts. Bounded by the ~30s retry
      // budget below, so worst case ~60 attempts total.
      timeoutId = setTimeout(() => { void tryConnect() }, 500)
    }

    void tryConnect()

    return () => {
      cancelled = true
      if (timeoutId !== null) clearTimeout(timeoutId)
    }
  }, [])

  if (!connected) {
    return <ConnectingOverlay attempts={attempts} />
  }

  return <>{children}</>
}

interface ConnectingOverlayProps {
  attempts: number
}

function ConnectingOverlay({ attempts }: ConnectingOverlayProps): React.ReactElement {
  // After ~30s of failed polls (60 attempts at 500ms), surface a
  // Reload button so the user can recover from a stuck state instead
  // of staring at an infinite spinner. Most auto-update restarts
  // complete in 1-3s; if we're at 30s, something's genuinely wrong.
  const showReloadButton = attempts >= 60

  return (
    <div
      style={{
        width: '100vw',
        height: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
        gap: '1.5rem',
        background: 'var(--color-bg, #0a0a0a)',
        color: 'var(--color-text-primary, #e0e0e0)',
        fontFamily: 'system-ui, -apple-system, sans-serif',
        userSelect: 'none',
        WebkitUserSelect: 'none',
        cursor: 'default',
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.5rem' }}>
        <div style={{ fontSize: '1rem', fontWeight: 500 }}>
          Connecting…
        </div>
        {attempts >= 4 && (
          <div style={{ fontSize: '0.75rem', opacity: 0.55, fontFamily: 'ui-monospace, monospace' }}>
            Waiting for K2SO daemon (attempt {attempts + 1})
          </div>
        )}
      </div>
      {showReloadButton && (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.5rem' }}>
          <div style={{ fontSize: '0.85rem', opacity: 0.7, maxWidth: '400px', textAlign: 'center', lineHeight: 1.5 }}>
            The daemon isn't responding. This usually clears on its own — try reloading.
          </div>
          <button
            onClick={() => { window.location.reload() }}
            style={{
              padding: '0.5rem 1rem',
              fontSize: '0.85rem',
              borderRadius: '4px',
              border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
              background: 'var(--color-bg-elevated, rgba(255,255,255,0.05))',
              color: 'inherit',
              cursor: 'pointer',
            }}
          >
            Reload
          </button>
        </div>
      )}
    </div>
  )
}

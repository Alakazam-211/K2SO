/**
 * ConnectionGate — wraps the React app and gates BOTH render AND module
 * import on daemon reachability.
 *
 * Why this exists (0.39.2 → fixed in 0.39.3):
 *
 * 0.39.2 shipped a gate that only delayed the *render* of <App />.
 * That was insufficient: <App /> imports a long list of Zustand
 * stores (projects, tabs, settings, focus-groups, timer, assistant,
 * panels, …) and several of those stores fire eager daemon fetches
 * at import time (not at component mount time). When index.tsx
 * statically imported <App /> at startup, those stores burned their
 * initial fetches against a down daemon, ended up in stuck/failed
 * state, and stayed broken even after the gate dismissed — App
 * mounted against empty stores and rendered as a black window.
 *
 * 0.39.3 fixes this by deferring the *import* of <App /> until the
 * daemon is verified healthy. The gate uses dynamic `import('./App')`
 * so the entire App module tree (and its transitively-imported
 * stores) doesn't enter the JS context until /ping succeeds. Stores
 * therefore fire their initial fetches against a known-healthy
 * daemon, no race, no stuck state.
 *
 * Same primitive is reusable for K2 Connect: a remote daemon
 * (Machine A's daemon, accessed from Machine B over a tunnel) may
 * be transiently unreachable. Gate behaviour is identical — show
 * "Connecting…", retry, mount app once reachable.
 *
 * Happy path: /ping succeeds on first try, overlay flashes for
 * <100ms (or skips entirely), user sees the app mount normally.
 *
 * Auto-update / cold-start race path: /ping fails for 1-3s, gate
 * shows "Connecting…", mounts the app once /ping responds — App
 * module imports happen NOW, stores fire fetches against the
 * healthy daemon, render succeeds cleanly.
 *
 * Permanent-failure path: after ~10s of failed polls, a Reload
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

type AppComponent = React.ComponentType

export function ConnectionGate(): React.ReactElement {
  const [connected, setConnected] = useState(false)
  const [attempts, setAttempts] = useState(0)
  const [AppModule, setAppModule] = useState<AppComponent | null>(null)

  // Phase 1: poll daemon until reachable.
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
      timeoutId = setTimeout(() => { void tryConnect() }, 500)
    }

    void tryConnect()

    return () => {
      cancelled = true
      if (timeoutId !== null) clearTimeout(timeoutId)
    }
  }, [])

  // Phase 2: once daemon is reachable, dynamically import App.
  // The import side-effects (store creation, eager fetches) only
  // run NOW, after the daemon is confirmed healthy.
  useEffect(() => {
    if (!connected) return
    let cancelled = false
    void import('../App').then((mod) => {
      if (cancelled) return
      // Wrap in a function returning the component so React's
      // useState setter doesn't call it as a setter callback.
      setAppModule(() => mod.default)
    }).catch((err: unknown) => {
      // Dynamic import failure (network error, bundle missing).
      // Keep showing the overlay; user can hit Reload.
      console.error('[ConnectionGate] dynamic import of App failed:', err)
    })
    return () => { cancelled = true }
  }, [connected])

  // Show overlay while either (a) daemon not reachable, or
  // (b) App module hasn't finished loading yet.
  if (!connected || AppModule === null) {
    return <ConnectingOverlay attempts={attempts} />
  }

  // App module is loaded — mount it. Store fetches will fire from
  // module-init effects (which run NOW for the first time) against
  // the healthy daemon.
  const App = AppModule
  return <App />
}

interface ConnectingOverlayProps {
  attempts: number
}

function ConnectingOverlay({ attempts }: ConnectingOverlayProps): React.ReactElement {
  // After ~10s of failed polls (20 attempts at 500ms), surface a
  // Reload button so the user can recover from a stuck state instead
  // of staring at an infinite spinner. Most auto-update restarts
  // complete in 1-3s; if we're at 10s, something's genuinely wrong.
  const showReloadButton = attempts >= 20

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
      <div style={{ fontSize: '1rem', fontWeight: 500 }}>
        Connecting…
      </div>
      {showReloadButton && (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.75rem' }}>
          <div style={{ fontSize: '0.85rem', opacity: 0.75, maxWidth: '440px', textAlign: 'center', lineHeight: 1.5 }}>
            Your K2SO daemon may still be loading.
            <br />
            If you're unsure, quit and relaunch the app, or try reloading with the button below.
          </div>
          <button
            onClick={() => { window.location.reload() }}
            style={{
              padding: '0.5rem 1.25rem',
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

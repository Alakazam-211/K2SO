/**
 * ConnectionGate — gates BOTH render AND module import of the app on the
 * daemon being (a) the build paired with THIS app and (b) finished with
 * its first-boot migrations.
 *
 * ## Why this exists
 *
 * 0.39.2/0.39.3 deferred the import of <App /> until the daemon answered
 * /ping, so its eager store fetches wouldn't fire against a down daemon.
 * But /ping only proves *something* is listening — and during a
 * 0.38.x → 0.39.x auto-update the OUTGOING old daemon was still bound to
 * the stable port answering /ping while the new daemon was being
 * kickstarted and grinding through its (heavy, one-time) first-boot
 * migration. The gate took that false-positive ping, mounted the app,
 * and its fetches hit the gap where the old daemon had been killed and
 * the new one wasn't serving yet → blank window ("appears to have
 * crashed").
 *
 * ## The fix (0.39.5)
 *
 * The daemon now binds its port BEFORE migrating and exposes a versioned
 * readiness handshake at GET /boot-status:
 *
 *     { version, protocol, phase, detail }
 *
 * This gate polls /boot-status and only mounts when an **acceptance
 * policy** says so. The local/auto-update policy ([`localPairedPolicy`])
 * requires `version === this app's bundled version` AND `phase ===
 * 'ready'` — so it can never bind to the outgoing old daemon (which
 * either reports an older version or, pre-0.39.5, 404s /boot-status
 * entirely), and it can render the migration progress (`detail`) to the
 * user instead of a blank window.
 *
 * ## Future-proofing (K2 Connect)
 *
 * The gate core is version-agnostic: the version/protocol decision lives
 * entirely in the injected policy. K2 Connect, which legitimately talks
 * to a remote daemon of a *different* marketing version, will supply a
 * different policy that range-checks `protocol` instead of requiring
 * exact `version` equality — without touching this component. Keep that
 * logic in the policy, never inline. See
 * [[project_daemon_handshake_contract]].
 */
import React, { useEffect, useRef, useState } from 'react'
import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useConnectHostStore } from '@/stores/connect-host'

/** Shape of the daemon's GET /boot-status response. `detail` is free-text
 *  for the UI only — never branch on it. */
interface DaemonBootStatus {
  version: string
  protocol: number
  phase: string // 'starting' | 'migrating' | 'ready' | 'error' | future
  detail: string
}

/** The gate's verdict for a single poll. */
type GateDecision =
  | { kind: 'accept' }
  | { kind: 'migrating'; detail: string }
  | { kind: 'wait'; reason: string } // unreachable / wrong version / old daemon

/** Decides whether to mount the app against a daemon, given its
 *  /boot-status (or null when unreachable / 404 / unparseable). */
interface AcceptancePolicy {
  decide(status: DaemonBootStatus | null): GateDecision
}

/**
 * Local auto-update / startup policy: only accept the daemon BUILT AND
 * SHIPPED with this app. `expectedVersion` is the app's bundled version
 * (from Tauri's getVersion()); the release script keeps it in lockstep
 * with the daemon's CARGO_PKG_VERSION, so exact equality is the correct
 * pairing check.
 *
 * If `expectedVersion` is null (non-Tauri/dev context where getVersion()
 * is unavailable) we fall back to readiness-only — still safe, because a
 * pre-0.39.5 daemon has no /boot-status and surfaces as `null` → wait.
 *
 * NOTE: this exact-equality is deliberately confined here. K2 Connect
 * must NOT reuse it — a remote daemon can be a different version. See the
 * file header.
 */
function localPairedPolicy(expectedVersion: string | null): AcceptancePolicy {
  return {
    decide(status: DaemonBootStatus | null): GateDecision {
      if (!status) {
        // Unreachable, or a pre-0.39.5 daemon that 404s /boot-status —
        // i.e. the outgoing old daemon during an update. Keep waiting.
        return { kind: 'wait', reason: 'unreachable-or-legacy-daemon' }
      }
      if (expectedVersion && status.version !== expectedVersion) {
        // A daemon answered, but it's not the one paired with this app
        // (e.g. an older daemon still up mid-update). Never mount against
        // it — wait for the kickstarted, correctly-versioned daemon.
        return { kind: 'wait', reason: `version ${status.version} != app ${expectedVersion}` }
      }
      if (status.phase !== 'ready') {
        // Correct daemon, still finishing first-boot migrations. Show the
        // user what's happening instead of a blank screen.
        return { kind: 'migrating', detail: status.detail }
      }
      return { kind: 'accept' }
    },
  }
}

/** Resolve this app's bundled version via Tauri. Returns null outside a
 *  Tauri context (e.g. a plain browser dev server) so the gate degrades
 *  to readiness-only rather than hanging. */
async function getAppVersion(): Promise<string | null> {
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    return await getVersion()
  } catch {
    return null
  }
}

/** Hit the daemon's /boot-status with a short per-attempt timeout.
 *  Returns the parsed status, or null on any error / non-2xx (covers a
 *  pre-0.39.5 daemon's 404, network error, timeout, missing port file). */
async function fetchBootStatus(): Promise<DaemonBootStatus | null> {
  try {
    // Host-aware (K2 Connect step #1): polls the ACTIVE host's
    // /boot-status. For 'local' this is byte-identical to before
    // (host === '127.0.0.1').
    const creds = await getDaemonWs()
    const resp = await fetch(`${daemonHttpBase(creds)}/boot-status`, {
      signal: AbortSignal.timeout(2000),
    })
    if (!resp.ok) {
      // 404 ⇒ pre-0.39.5 daemon (no /boot-status route). Re-read the port
      // file next poll in case a kickstart moved it.
      invalidateDaemonWs()
      return null
    }
    return (await resp.json()) as DaemonBootStatus
  } catch {
    // Network error, timeout, port file missing, etc. — daemon isn't
    // reachable yet. Invalidate cached port so the next poll re-reads
    // ~/.k2so/daemon.port (covers a kickstart-assigned port change).
    invalidateDaemonWs()
    return null
  }
}

type AppComponent = React.ComponentType

/** A stable identity for the active host — changes whenever the user
 *  switches daemons. Drives a gate re-poll + a clean App remount so all
 *  live sockets re-establish against the new host. */
function activeHostKey(active: ReturnType<typeof useConnectHostStore.getState>['activeHost']): string {
  return active === 'local' ? 'local' : `${active.id}:${active.hostname}:${active.port}`
}

export function ConnectionGate(): React.ReactElement {
  const [decision, setDecision] = useState<GateDecision>({ kind: 'wait', reason: 'starting' })
  const [attempts, setAttempts] = useState(0)
  const [AppModule, setAppModule] = useState<AppComponent | null>(null)
  const policyRef = useRef<AcceptancePolicy | null>(null)

  // K2 Connect step #1: the gate is host-aware. `hostKey` changes when
  // the user picks a different daemon in the top-bar switcher → the
  // polling effect below re-runs against the new host, and the <App>
  // element is keyed by it so a switch remounts the app cleanly (all
  // sockets re-open through the new host's creds).
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hostKey = activeHostKey(activeHost)

  // Phase 1: resolve the app version once, then poll the ACTIVE host's
  // /boot-status until the acceptance policy says to mount. Re-runs when
  // the active host changes (hostKey dep).
  useEffect(() => {
    let cancelled = false
    let timeoutId: ReturnType<typeof setTimeout> | null = null

    // A host switch must re-poll from scratch: drop any prior accept so
    // the overlay shows while the new host is contacted.
    setDecision({ kind: 'wait', reason: 'switching-host' })
    setAttempts(0)

    const ensurePolicy = async (): Promise<AcceptancePolicy> => {
      // KEEP the localPairedPolicy for now (step #1/#2). A SAME-VERSION
      // second daemon passes it, which is how a remote is tested today.
      // TODO(#4): inject a remote protocol-range policy when activeHost
      // is a ConnectHost.
      if (!policyRef.current) {
        const version = await getAppVersion()
        policyRef.current = localPairedPolicy(version)
      }
      return policyRef.current
    }

    const tick = async (): Promise<void> => {
      const policy = await ensurePolicy()
      const status = await fetchBootStatus()
      if (cancelled) return
      const next = policy.decide(status)
      setDecision(next)
      // Surface the active host's live status to the top-bar switcher.
      useConnectHostStore.getState().setConnectionStatus(
        next.kind === 'accept' ? 'connected' : 'connecting',
      )
      if (next.kind === 'accept') return // stop polling; Phase 2 takes over
      setAttempts((a) => a + 1)
      timeoutId = setTimeout(() => { void tick() }, 500)
    }

    void tick()

    return () => {
      cancelled = true
      if (timeoutId !== null) clearTimeout(timeoutId)
    }
  }, [hostKey])

  // Phase 2: once accepted, dynamically import App. Its import
  // side-effects (store creation, eager fetches) run NOW for the first
  // time, against a daemon confirmed to be the right version AND ready.
  useEffect(() => {
    if (decision.kind !== 'accept') return
    let cancelled = false
    void import('../App').then((mod) => {
      if (cancelled) return
      setAppModule(() => mod.default)
    }).catch((err: unknown) => {
      console.error('[ConnectionGate] dynamic import of App failed:', err)
    })
    return () => { cancelled = true }
  }, [decision.kind])

  if (decision.kind !== 'accept' || AppModule === null) {
    return <ConnectingOverlay decision={decision} attempts={attempts} />
  }

  const App = AppModule
  // Key by the active host so switching daemons unmounts + remounts App
  // wholesale — every store, WS, and terminal pane re-initializes against
  // the new host's creds rather than clinging to the old socket.
  return <App key={hostKey} />
}

interface ConnectingOverlayProps {
  decision: GateDecision
  attempts: number
}

function ConnectingOverlay({ decision, attempts }: ConnectingOverlayProps): React.ReactElement {
  const migrating = decision.kind === 'migrating'

  // Heading + sub-line. While the (correct) daemon is migrating we tell
  // the user updates are being applied; otherwise it's a plain connect.
  const heading = migrating ? 'Setting up K2SO…' : 'Connecting…'
  const subline = migrating
    ? (decision.detail && decision.detail.length > 0 ? decision.detail : 'Applying updates…')
    : null

  // Reload escape hatch. A big upgrade's migration sweep can legitimately
  // take a while, so don't nag during 'migrating' (only after ~60s). For
  // a plain 'wait' (unreachable / wrong version) surface it after ~10s.
  const showReloadButton = migrating ? attempts >= 120 : attempts >= 20

  return (
    <div
      style={{
        width: '100vw',
        height: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
        gap: '1.25rem',
        background: 'var(--color-bg, #0a0a0a)',
        color: 'var(--color-text-primary, #e0e0e0)',
        fontFamily: 'system-ui, -apple-system, sans-serif',
        userSelect: 'none',
        WebkitUserSelect: 'none',
        cursor: 'default',
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.5rem' }}>
        <div style={{ fontSize: '1rem', fontWeight: 500 }}>{heading}</div>
        {subline !== null && (
          <div style={{ fontSize: '0.85rem', opacity: 0.7 }}>{subline}</div>
        )}
      </div>
      {showReloadButton && (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '0.75rem' }}>
          <div style={{ fontSize: '0.85rem', opacity: 0.75, maxWidth: '440px', textAlign: 'center', lineHeight: 1.5 }}>
            {migrating
              ? 'K2SO is still applying updates. This can take a minute on a large upgrade — you can keep waiting, or reload below.'
              : "Your K2SO daemon may still be loading. If you're unsure, quit and relaunch the app, or try reloading with the button below."}
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

// Cached accessor for `invoke('daemon_ws_url')`.
//
// The underlying Tauri command reads two files from `~/.k2so/` on
// every call (heartbeat.port + heartbeat.token). That's ~5-10ms of
// synchronous disk I/O. Every Kessel pane mount was paying the cost
// independently — a visible delay contributor on top of the
// spawn + WS handshake.
//
// The port + token never change during an app session: the daemon
// writes them once at boot and the files are stable until the daemon
// is restarted (which kills this app anyway, via heartbeat). So one
// invoke per app process is enough.
//
// Usage:
//   await prewarmDaemonWs()               // call at app startup
//   const { port, token } = await getDaemonWs() // cheap on subsequent calls
//
// On error the cache invalidates so the next caller retries — the
// daemon may have been briefly unavailable (e.g. a reinstall).

import { invoke } from '@tauri-apps/api/core'
import { useConnectHostStore } from '@/stores/connect-host'

/** Loopback host the local bundled daemon always binds. When the active
 *  host is 'local' the resolved creds carry exactly this — so every URL
 *  built from `creds.host` is byte-identical to the pre-host-aware code
 *  that hardcoded `127.0.0.1`. */
const LOCAL_HOST = '127.0.0.1'

export type DaemonWsState =
  | { state: 'available'; port: number; token: string }
  | { state: 'not_installed'; reason?: string }

export interface DaemonWsAvailable {
  port: number
  token: string
  /** Daemon host. `'127.0.0.1'` for the local daemon (byte-identical to
   *  before); the active ConnectHost's hostname when remote. */
  host: string
}

interface RawResponse {
  state: 'available' | 'not_installed'
  port?: number
  token?: string
  reason?: string
}

let cached: Promise<DaemonWsAvailable> | null = null

/** Invalidate the cache so the next call re-invokes the Tauri command.
 *  Call this if a WS connection repeatedly fails or the token is
 *  rejected — daemon may have been restarted and reissued credentials. */
export function invalidateDaemonWs(): void {
  cached = null
}

/** Resolve to {port, token, host}. Host-aware (K2 Connect step #1):
 *
 *   - `activeHost === 'local'` → resolves via the Tauri `daemon_ws_url`
 *     command EXACTLY as before, with `host: '127.0.0.1'`. The result is
 *     cached per app session (the local daemon's creds are stable).
 *   - `activeHost` is a ConnectHost → resolves directly from the saved
 *     `{hostname, port, token}`. NOT cached against the local-creds
 *     `cached` promise, because the active host can change at runtime;
 *     deriving from the store each call keeps switches instant.
 *
 * Rejects with a message when the local daemon isn't reachable — the
 * reject invalidates the local cache so recovery is just a retry. */
export function getDaemonWs(): Promise<DaemonWsAvailable> {
  const active = useConnectHostStore.getState().activeHost
  if (active !== 'local') {
    // Remote host: derive creds directly from the store. No Tauri
    // invoke, no shared cache (a switch back to local must not return
    // stale remote creds and vice-versa).
    return Promise.resolve({
      port: active.port,
      token: active.token,
      host: active.hostname,
    })
  }
  if (cached) return cached
  cached = (async () => {
    let res: RawResponse
    try {
      res = await invoke<RawResponse>('daemon_ws_url')
    } catch (e) {
      cached = null
      throw new Error(`daemon_ws_url invoke failed: ${String(e)}`)
    }
    if (res.state !== 'available' || !res.port || !res.token) {
      cached = null
      throw new Error(`daemon not reachable: ${res.reason ?? 'unknown'}`)
    }
    return { port: res.port, token: res.token, host: LOCAL_HOST }
  })()
  return cached
}

/**
 * Build a daemon HTTP URL from resolved creds. Centralizes the
 * `http://<host>:<port>` construction so call sites don't hardcode
 * `127.0.0.1`. `path` should start with `/` (e.g. `/boot-status`,
 * `/cli/sessions/v2/spawn`). For local, `creds.host === '127.0.0.1'`
 * so the output is byte-identical to the old literals.
 *
 * TODO(#4): remotes reached over the k2.dev tunnel terminate TLS at
 * Caddy → those will want `https://`. Direct-IP LAN remotes stay
 * `http://`. Scheme selection lands with the remote AcceptancePolicy.
 */
export function daemonHttpBase(creds: DaemonWsAvailable): string {
  return `http://${creds.host}:${creds.port}`
}

/** Build a daemon WS URL base (`ws://<host>:<port>`). See
 *  {@link daemonHttpBase}; same TLS caveat (TODO #4 → `wss://`). */
export function daemonWsBase(creds: DaemonWsAvailable): string {
  return `ws://${creds.host}:${creds.port}`
}

/** Fire-and-forget warm-up. Safe to call from app mount; errors are
 *  swallowed because the terminal-pane code paths call `getDaemonWs`
 *  themselves and will surface a real error at that point if the
 *  daemon isn't reachable.
 *
 *  0.39.0: the Rust-side `kessel_warm_http` companion call was
 *  removed along with the Kessel renderer (the v1 spawn path was
 *  retired; daemon spawns now go through the alacritty-v2 WS handler
 *  which doesn't share the blocking reqwest runtime). This function
 *  is now just a creds-cache primer — kept so callers don't have to
 *  drop the no-op invocation. */
export function prewarmDaemonWs(): void {
  // Resolve cached creds. Subsequent getDaemonWs() callers reuse the
  // cached promise so the disk read only happens once per app session.
  getDaemonWs().catch(() => {
    /* daemon not ready yet — real subscribers will retry from disk */
  })
}

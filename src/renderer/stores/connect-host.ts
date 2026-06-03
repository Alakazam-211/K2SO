// connect-host store — the host-aware connection substrate for K2 Connect
// (PRD: .k2so/prds/k2-connect-client-ux.md, build order step #1).
//
// The desktop client can target ANY daemon: the local bundled daemon
// ("This Mac"), a self-hosted box, or a hosted K2 Connect server. This
// store holds WHICH daemon the renderer is currently talking to and the
// address book of saved hosts.
//
// ## What lives here now (step #1/#2)
//   - `activeHost`: 'local' | ConnectHost  (default 'local')
//   - `hosts`: ConnectHost[]               (the saved address book)
//   - selectHost / addHost / removeHost mutators
//   - localStorage persistence of the host list MINUS the token.
//
// ## What is deliberately deferred
//   - Keychain-backed token storage + ~/.k2so/connect-hosts.json
//     (step #3). For now the non-secret host list persists to
//     localStorage and tokens are held IN MEMORY only — a remembered
//     token survives a host switch but NOT an app restart yet.
//   - Settings → Connections address-book UI (step #3).
//   - Remote AcceptancePolicy / soft-reconnect / latency readout
//     (steps #4/#5).
//
// ## Local is byte-identical
// When `activeHost === 'local'`, daemon-ws.ts resolves the exact same
// Tauri `daemon_ws_url` path it always has. This store is purely
// additive — a fresh install with no saved hosts behaves exactly as
// today.

import { create } from 'zustand'

/**
 * A saved K2 server the client can connect to. Mirrors the Phase 3 PRD
 * §E.3 schema (with the §3 correction that the token does NOT live in
 * the persisted JSON — it goes to the keychain in step #3; for now it's
 * memory-only).
 */
export interface ConnectHost {
  /** Stable client-generated id (used as the dropdown key + removeHost arg). */
  id: string
  /** User-facing name shown in the switcher (e.g. "Hetzner box"). */
  label: string
  /** Hostname or IP of the daemon (no scheme, no port). */
  hostname: string
  /** Daemon port. */
  port: number
  /**
   * Auth token (rides as `?token=`). SECRET — never persisted to
   * localStorage. Held in memory for the session; step #3 moves the
   * remembered variant to the OS keychain.
   */
  token: string
  /**
   * "Remember password" toggle. When true, step #3 will persist the
   * token to the keychain. Until then this only records intent; the
   * token is memory-only regardless.
   */
  remember: boolean
  /** Epoch ms of the last successful connect, or null if never. */
  lastConnectedAt: number | null
}

/** The non-secret shape we persist to localStorage (everything except
 *  `token`). */
type PersistedHost = Omit<ConnectHost, 'token'>

export type ActiveHost = 'local' | ConnectHost

/**
 * Live connection state for the ACTIVE host, driven by ConnectionGate's
 * boot-status poll. The switcher renders a status dot from this.
 *   - 'connecting' → gate is polling / app not yet mounted (or switching)
 *   - 'connected'  → gate accepted; app mounted against this host
 *   - 'offline'    → reserved seam (step #5 latency layer flips this on
 *     repeated poll failure; for now the gate only sets connecting/connected)
 */
export type ConnectionStatus = 'connecting' | 'connected' | 'offline'

interface ConnectHostState {
  activeHost: ActiveHost
  hosts: ConnectHost[]
  /** Live status of the active host's connection (set by the gate). */
  connectionStatus: ConnectionStatus
  /** Switch the active daemon. Pass 'local' for This Mac, or a saved
   *  ConnectHost. Selecting a host stamps `lastConnectedAt` optimistically
   *  is left to the gate (step #4); for now we just re-point. */
  selectHost: (hostOrLocal: ActiveHost) => void
  /** Add (or replace by id) a host in the address book. */
  addHost: (host: ConnectHost) => void
  /** Remove a host by id. If it's the active host, fall back to 'local'. */
  removeHost: (id: string) => void
  /** Gate-driven: update the live connection status of the active host. */
  setConnectionStatus: (status: ConnectionStatus) => void
}

const STORAGE_KEY = 'k2so.connect-hosts.v1'

/** Best-effort access to localStorage. Returns null in non-browser
 *  (vitest node) contexts so the store still works headless. */
function getStorage(): Storage | null {
  try {
    if (typeof localStorage !== 'undefined') return localStorage
  } catch {
    /* SecurityError in some sandboxes — treat as unavailable */
  }
  return null
}

/** Load the persisted (token-less) host list. Tokens are NOT persisted,
 *  so loaded hosts start with an empty token + remember preserved. */
function loadHosts(): ConnectHost[] {
  const storage = getStorage()
  if (!storage) return []
  const raw = storage.getItem(STORAGE_KEY)
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((h): h is PersistedHost => isPersistedHost(h))
      .map((h) => ({ ...h, token: '' }))
  } catch {
    return []
  }
}

function isPersistedHost(h: unknown): h is PersistedHost {
  if (typeof h !== 'object' || h === null) return false
  const o = h as Record<string, unknown>
  return (
    typeof o.id === 'string' &&
    typeof o.label === 'string' &&
    typeof o.hostname === 'string' &&
    typeof o.port === 'number' &&
    typeof o.remember === 'boolean' &&
    (o.lastConnectedAt === null || typeof o.lastConnectedAt === 'number')
  )
}

/** Persist the host list MINUS the token. This is the security
 *  invariant for step #1: secrets never touch localStorage. */
function persistHosts(hosts: ConnectHost[]): void {
  const storage = getStorage()
  if (!storage) return
  const sansToken: PersistedHost[] = hosts.map(({ token: _token, ...rest }) => rest)
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(sansToken))
  } catch {
    /* quota / disabled storage — non-fatal, hosts stay in memory */
  }
}

export const useConnectHostStore = create<ConnectHostState>((set, get) => ({
  activeHost: 'local',
  hosts: loadHosts(),
  connectionStatus: 'connecting',

  selectHost: (hostOrLocal) => {
    // A switch immediately re-enters the connecting state; the gate
    // flips it to 'connected' once the new host accepts.
    set({ activeHost: hostOrLocal, connectionStatus: 'connecting' })
  },

  setConnectionStatus: (status) => {
    set({ connectionStatus: status })
  },

  addHost: (host) => {
    const existing = get().hosts
    // Replace-by-id so re-adding/editing an existing entry updates in
    // place rather than duplicating.
    const without = existing.filter((h) => h.id !== host.id)
    const next = [...without, host]
    persistHosts(next)
    set({ hosts: next })
  },

  removeHost: (id) => {
    const { hosts, activeHost } = get()
    const next = hosts.filter((h) => h.id !== id)
    persistHosts(next)
    // If we removed the currently-active host, fall back to local.
    const nextActive: ActiveHost =
      activeHost !== 'local' && activeHost.id === id ? 'local' : activeHost
    set({ hosts: next, activeHost: nextActive })
  },
}))

/** Test-only: reset the store to defaults and clear persisted hosts.
 *  Mirrors the `__reset*ForTests` hooks other stores expose. */
export function __resetConnectHostStoreForTests(): void {
  const storage = getStorage()
  storage?.removeItem(STORAGE_KEY)
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connecting' })
}

/** The localStorage key, exported for tests. */
export const CONNECT_HOSTS_STORAGE_KEY = STORAGE_KEY

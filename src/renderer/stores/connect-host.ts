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
// ## Persistence (step #3 — landed)
//   - The NON-SECRET host list persists to `~/.k2so/connect-hosts.json`
//     via the `connect_hosts_{read,write}` Tauri commands (localStorage
//     is kept as a synchronous boot cache / non-Tauri fallback).
//   - REMEMBERED tokens persist to the OS keychain via
//     `k2_secret_{set,get,delete}` (service `K2_CONNECT_KEYCHAIN_SERVICE`,
//     account = host.id). Non-remembered tokens stay in memory only.
//   - `hydrateFromDisk()` runs once at boot: loads the file then resolves
//     each remembered host's token from the keychain. Call it from the
//     app shell.
//
// ## What is deliberately deferred
//   - Remote AcceptancePolicy / soft-reconnect / latency readout
//     (steps #4/#5) — partially landed in the ConnectionGate work.
//
// ## Local is byte-identical
// When `activeHost === 'local'`, daemon-ws.ts resolves the exact same
// Tauri `daemon_ws_url` path it always has. This store is purely
// additive — a fresh install with no saved hosts behaves exactly as
// today.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'

/**
 * Keychain service name for remembered remote-host tokens. Each token is
 * stored under `(K2_CONNECT_KEYCHAIN_SERVICE, host.id)` via the
 * `k2_secret_*` Tauri commands. Exported so the Settings → Connections UI
 * can store/forget tokens with the SAME key the store reads on boot.
 */
export const K2_CONNECT_KEYCHAIN_SERVICE = 'com.k2so.connect.host-token'

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
  /** Daemon port. For a secure hosted remote this is typically 443
   *  (and the port is omitted from the built URL). */
  port: number
  /**
   * TLS (K2 Connect step #4). When true, daemon-ws.ts builds
   * `https://`/`wss://` URLs and omits port 443 — for a hosted tunnel
   * like `rosson.k2.dev` where Caddy terminates TLS. Defaults to true
   * for non-local hostnames added via the switcher; false for
   * localhost/LAN direct-IP. Non-secret, so it persists to localStorage.
   */
  secure: boolean
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

/** A loopback / LAN-localhost hostname speaks plain HTTP — never TLS.
 *  Used to pick the `secure` default when adding a host: non-local
 *  hostnames (e.g. `rosson.k2.dev`) default to secure (TLS via the
 *  tunnel), localhost/127.0.0.1/::1 default to plain. */
export function isLocalHostname(hostname: string): boolean {
  const h = hostname.trim().toLowerCase()
  return (
    h === 'localhost' ||
    h === '127.0.0.1' ||
    h === '::1' ||
    h === '[::1]' ||
    h === '0.0.0.0'
  )
}

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
  /**
   * A host the user picked that needs the full-screen sign-in before it
   * can become active — i.e. it has no remembered/in-memory token, or a
   * previously-remembered one was rejected/expired. Null = no sign-in
   * pending. The sign-in overlay (mounted by ConnectionGate) reads this;
   * a remembered host with a resolved token bypasses it (silent auto
   * sign-in via `selectHost`). */
  pendingSignIn: ConnectHost | null
  /** Switch the active daemon. Pass 'local' for This Mac, or a saved
   *  ConnectHost. Selecting a host stamps `lastConnectedAt` optimistically
   *  is left to the gate (step #4); for now we just re-point. */
  selectHost: (hostOrLocal: ActiveHost) => void
  /** Add (or replace by id) a host in the address book. */
  addHost: (host: ConnectHost) => void
  /** Remove a host by id. If it's the active host, fall back to 'local'.
   *  Also forgets any keychain token for that host. */
  removeHost: (id: string) => void
  /** Gate-driven: update the live connection status of the active host. */
  setConnectionStatus: (status: ConnectionStatus) => void
  /** Set a host's in-memory token (e.g. after a successful sign-in) so
   *  daemon-ws.ts can use it for the active connection. Does NOT touch
   *  the keychain — call `rememberToken`/`forgetToken` for that. */
  setHostToken: (id: string, token: string) => void
  /** Boot-time hydration: load the durable host list from
   *  ~/.k2so/connect-hosts.json, then resolve each REMEMBERED host's
   *  token from the keychain into memory. Idempotent; call once from the
   *  app shell. No-op (resolves) outside Tauri. */
  hydrateFromDisk: () => Promise<void>
  /** Open the full-screen sign-in for `host` (no/expired token). */
  requestSignIn: (host: ConnectHost) => void
  /** Dismiss the full-screen sign-in without switching. */
  cancelSignIn: () => void
  /**
   * Pick a host the user selected: if it already has a usable in-memory
   * token, switch to it directly (silent). Otherwise open the full-screen
   * sign-in. 'local' always switches directly (never needs auth). This is
   * the single entry point the switcher + auto-sign-in both call. */
  pickHost: (hostOrLocal: ActiveHost) => void
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
      // `secure` may be absent in entries persisted before step #4 —
      // default to false (plain http/ws) so old saved hosts keep their
      // prior behaviour. Token is never persisted; starts empty.
      .map((h) => ({ ...h, secure: h.secure ?? false, token: '' }))
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
    // `secure` optional for backward-compat (loadHosts defaults it to
    // false); reject only an explicitly wrong type.
    (o.secure === undefined || typeof o.secure === 'boolean') &&
    (o.lastConnectedAt === null || typeof o.lastConnectedAt === 'number')
  )
}

/** Persist the host list MINUS the token. This is the security
 *  invariant: secrets never touch localStorage OR connect-hosts.json.
 *
 *  Two backends:
 *    1. localStorage — synchronous, so the next boot has the list
 *       immediately (the switcher renders before async hydration lands).
 *    2. `~/.k2so/connect-hosts.json` via the `connect_hosts_write` Tauri
 *       command — the durable, daemon/CLI-visible source of truth. Fire-
 *       and-forget; a failure leaves localStorage as the fallback. */
function persistHosts(hosts: ConnectHost[]): void {
  const sansToken: PersistedHost[] = hosts.map(({ token: _token, ...rest }) => rest)
  const json = JSON.stringify(sansToken)

  const storage = getStorage()
  if (storage) {
    try {
      storage.setItem(STORAGE_KEY, json)
    } catch {
      /* quota / disabled storage — non-fatal, hosts stay in memory + file */
    }
  }

  // Durable file write (Tauri only). Never blocks the caller.
  void invoke('connect_hosts_write', { json }).catch(() => {
    /* non-Tauri (vitest/web) or IO failure — localStorage covers boot */
  })
}

// ── Keychain helpers (remembered-token persistence) ─────────────────────
// Tokens for "Remember password" hosts live in the OS keychain, NEVER in
// localStorage or connect-hosts.json. Keyed by host id. All three are
// best-effort + Tauri-only; they no-op (resolve) in the vitest/web env.

/** Store a host's token in the keychain under its id. */
export async function rememberToken(hostId: string, token: string): Promise<void> {
  try {
    await invoke('k2_secret_set', {
      service: K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
      secret: token,
    })
  } catch {
    /* keychain unavailable — caller keeps the token in memory */
  }
}

/** Resolve a host's remembered token from the keychain, or null. */
export async function resolveToken(hostId: string): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', {
      service: K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
    })
    return secret ?? null
  } catch {
    return null
  }
}

/** Forget a host's remembered token (toggle-off / host removal). */
export async function forgetToken(hostId: string): Promise<void> {
  try {
    await invoke('k2_secret_delete', {
      service: K2_CONNECT_KEYCHAIN_SERVICE,
      account: hostId,
    })
  } catch {
    /* idempotent — a missing entry is fine */
  }
}

export const useConnectHostStore = create<ConnectHostState>((set, get) => ({
  activeHost: 'local',
  hosts: loadHosts(),
  connectionStatus: 'connecting',
  pendingSignIn: null,

  selectHost: (hostOrLocal) => {
    // A switch immediately re-enters the connecting state; the gate
    // flips it to 'connected' once the new host accepts. Clears any
    // pending sign-in (we're committing to a host now).
    set({ activeHost: hostOrLocal, connectionStatus: 'connecting', pendingSignIn: null })
  },

  requestSignIn: (host) => {
    set({ pendingSignIn: host })
  },

  cancelSignIn: () => {
    set({ pendingSignIn: null })
  },

  pickHost: (hostOrLocal) => {
    if (hostOrLocal === 'local') {
      get().selectHost('local')
      return
    }
    // A usable in-memory token (resolved from the keychain on boot, or
    // set during a prior sign-in this session) → switch silently.
    if (hostOrLocal.token && hostOrLocal.token.length > 0) {
      get().selectHost(hostOrLocal)
      return
    }
    // No token → full-screen sign-in for this specific host.
    get().requestSignIn(hostOrLocal)
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
    // Drop any remembered token from the keychain too (best-effort).
    void forgetToken(id)
    // If we removed the currently-active host, fall back to local.
    const nextActive: ActiveHost =
      activeHost !== 'local' && activeHost.id === id ? 'local' : activeHost
    set({ hosts: next, activeHost: nextActive })
  },

  setHostToken: (id, token) => {
    const { hosts, activeHost } = get()
    const hosts2 = hosts.map((h) => (h.id === id ? { ...h, token } : h))
    // Keep the active host object in sync if it's the one being updated,
    // so daemon-ws.ts (which reads activeHost.token) sees the new token.
    const active2: ActiveHost =
      activeHost !== 'local' && activeHost.id === id
        ? { ...activeHost, token }
        : activeHost
    set({ hosts: hosts2, activeHost: active2 })
  },

  hydrateFromDisk: async () => {
    // 1. Load the durable host list. Falls back to whatever the
    //    constructor already loaded from localStorage on failure.
    let fileHosts: ConnectHost[] | null = null
    try {
      const raw = await invoke<string>('connect_hosts_read')
      const parsed = JSON.parse(raw) as unknown
      if (Array.isArray(parsed)) {
        fileHosts = parsed
          .filter((h): h is PersistedHost => isPersistedHost(h))
          .map((h) => ({ ...h, secure: h.secure ?? false, token: '' }))
      }
    } catch {
      /* non-Tauri / read failure — keep the localStorage-seeded list */
    }

    const base = fileHosts ?? get().hosts

    // 2. Resolve remembered tokens from the keychain into memory.
    const resolved = await Promise.all(
      base.map(async (h) => {
        if (!h.remember) return h
        const token = await resolveToken(h.id)
        return token ? { ...h, token } : h
      }),
    )

    // Mirror the resolved list back to localStorage (token-stripped) so a
    // subsequent synchronous boot has the file's view.
    persistHosts(resolved)
    set({ hosts: resolved })
  },
}))

/** Test-only: reset the store to defaults and clear persisted hosts.
 *  Mirrors the `__reset*ForTests` hooks other stores expose. */
export function __resetConnectHostStoreForTests(): void {
  const storage = getStorage()
  storage?.removeItem(STORAGE_KEY)
  useConnectHostStore.setState({ activeHost: 'local', hosts: [], connectionStatus: 'connecting', pendingSignIn: null })
}

/** The localStorage key, exported for tests. */
export const CONNECT_HOSTS_STORAGE_KEY = STORAGE_KEY

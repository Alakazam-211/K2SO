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
 * Keychain service name for a remembered remote-host PASSWORD. Distinct
 * from the session-token service above: the password is the long-lived
 * credential the user types once; the token is the short-lived session
 * bearer obtained from `POST /cli/auth/login`. When "remember password"
 * is on we persist the password here (keyed by host id) so connect-time
 * auto-login needs no prompt; the session token still lives under
 * {@link K2_CONNECT_KEYCHAIN_SERVICE}. Neither ever touches
 * connect-hosts.json.
 */
export const K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE = 'com.k2so.connect.host-password'

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
  /**
   * Account username for `POST /cli/auth/login` (connect-users, #617).
   * NON-SECRET — persists to connect-hosts.json alongside hostname/port.
   * Optional for backward-compat with hosts saved before connect-users
   * (those used a raw host token and have no username); a remote added
   * via the connect-users flow always has one. The local host never has
   * a username (it uses the daemon token).
   */
  username?: string
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
   * SESSION token (rides as `?token=`). SECRET — never persisted to
   * localStorage or connect-hosts.json. Held in memory for the session.
   *
   * connect-users (#617): for a connect-users host this is the bearer
   * returned by `POST /cli/auth/login {username,password}` → it is NOT
   * the user's password. It's obtained at connect time (see
   * {@link loginToHost}) and cached in the keychain under
   * {@link K2_CONNECT_KEYCHAIN_SERVICE} via {@link rememberToken}.
   * A 401 from any `/cli/*` call means it expired → drop + re-login.
   *
   * Legacy hosts (no username) still carry a raw host token here.
   */
  token: string
  /**
   * "Remember password" toggle. When true the user's PASSWORD persists
   * to the keychain (under {@link K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE})
   * so connect-time auto-login needs no prompt; the session token is
   * also cached. When false, the password is entered each connect via
   * RemoteSignIn and never persisted.
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
   *  ConnectHost. Sets `connectionStatus:'connecting'` immediately (UI
   *  feedback), then flips `activeHost` only AFTER the proxy override
   *  resolves — closing the ordering race where the `<App>` remount's
   *  refetch could beat the override into place. Returns a promise that
   *  resolves once the flip has happened; callers may fire-and-forget. */
  selectHost: (hostOrLocal: ActiveHost) => Promise<void>
  /** Add (or replace by id) a host in the address book. */
  addHost: (host: ConnectHost) => void
  /** Remove a host by id. If it's the active host, fall back to 'local'.
   *  Also forgets any keychain token for that host. */
  removeHost: (id: string) => void
  /** Gate-driven: update the live connection status of the active host. */
  setConnectionStatus: (status: ConnectionStatus) => void
  /** Set a host's in-memory token (e.g. after a successful sign-in) so
   *  daemon-ws.ts can use it for the active connection. Does NOT touch
   *  the keychain — call `rememberToken`/`forgetToken` for that. When the
   *  updated host is the active remote, the proxy override is refreshed
   *  BEFORE the `activeHost` object is swapped, mirroring `selectHost`'s
   *  ordering guarantee. Returns a promise that resolves once that swap
   *  has happened. */
  setHostToken: (id: string, token: string) => Promise<void>
  /** Boot-time hydration: load the durable host list from
   *  ~/.k2so/connect-hosts.json, then resolve each REMEMBERED host's
   *  token from the keychain into memory. Idempotent; call once from the
   *  app shell. No-op (resolves) outside Tauri. */
  hydrateFromDisk: () => Promise<void>
  /** Open the full-screen sign-in for `host` (no/expired token). */
  requestSignIn: (host: ConnectHost) => void
  /**
   * connect-users (#617) session expiry: a `/cli/*` call to a remote host
   * returned 401, so its cached session token is stale. Drop the
   * in-memory + keychain session token (NOT the remembered password) and
   * re-trigger the full-screen sign-in for that host. No-op for 'local'
   * or a host id that isn't the active remote (a stale 401 from a prior
   * host must not interrupt the current one). Returns a promise that
   * resolves once the proxy override has been cleared and state flipped. */
  expireSession: (hostId: string) => Promise<void>
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
    // `username` optional (connect-users #617; absent on legacy raw-token
    // hosts). Reject only an explicitly wrong type.
    (o.username === undefined || typeof o.username === 'string') &&
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

// ── Keychain helpers (remembered-PASSWORD persistence) ──────────────────
// connect-users (#617): when "remember password" is on we cache the
// user's login password (NOT the session token) so connect-time
// auto-login needs no prompt. Same Tauri-only / best-effort contract as
// the token helpers, under a DISTINCT service so a host can have a
// remembered password and a separate cached session token.

/** Store a host's login password in the keychain under its id. */
export async function rememberPassword(hostId: string, password: string): Promise<void> {
  try {
    await invoke('k2_secret_set', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
      secret: password,
    })
  } catch {
    /* keychain unavailable — user re-enters the password next connect */
  }
}

/** Resolve a host's remembered login password from the keychain, or null. */
export async function resolvePassword(hostId: string): Promise<string | null> {
  try {
    const secret = await invoke<string | null>('k2_secret_get', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
    })
    return secret ?? null
  } catch {
    return null
  }
}

/** Forget a host's remembered login password (toggle-off / host removal). */
export async function forgetPassword(hostId: string): Promise<void> {
  try {
    await invoke('k2_secret_delete', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: hostId,
    })
  } catch {
    /* idempotent — a missing entry is fine */
  }
}

// ── Login (connect-users #617) ───────────────────────────────────────────

/** Build `<scheme>://<host>[:<port>]` for a host, matching daemon-ws.ts:
 *  secure+443 omits the port; everything else carries it. */
function hostBaseUrl(host: Pick<ConnectHost, 'hostname' | 'port' | 'secure'>): string {
  const scheme = host.secure ? 'https' : 'http'
  const authority = host.secure && host.port === 443 ? host.hostname : `${host.hostname}:${host.port}`
  return `${scheme}://${authority}`
}

/**
 * Push the active host into the Tauri→daemon proxy layer so the
 * HOST-UNAWARE `invoke('projects_list')`-style commands (which go through
 * `DaemonClient`) route to the SAME daemon as the host-aware
 * `daemonCli*`/WS calls. Without this the local-DB-backed proxy commands
 * stay pinned to 127.0.0.1 while the user is connected to a remote host.
 *
 * Mapping (identical authority rules to {@link hostBaseUrl} / daemon-ws):
 *   - `'local'`  → `base: null, token: null` → DaemonClient clears its
 *     override and reads `~/.k2so/daemon.{port,token}` (byte-identical to
 *     before).
 *   - ConnectHost → `base: <scheme>://<authority>` (443 omitted for
 *     secure), `token: <session token>`.
 *
 * AWAITED before every `activeHost` flip (selectHost / expireSession /
 * setHostToken), so the override is in place on the Rust side BEFORE the
 * gate sees the new host and remounts `<App>` (which re-fires
 * `fetchProjects()` and friends). Awaiting closes an ordering race: the
 * `activeHost` change drives `<App key={hostKey}>` to remount and refetch,
 * and if that refetch raced ahead of the override landing, a snap-back to
 * local would fetch against the dead remote (0 workspaces). We still guard
 * against the tokenless-remote case (no token → clear to local) so a
 * remote call can never fire without auth and earn an "Invalid or missing
 * auth token".
 *
 * Returns the invoke promise so callers can `await` it before flipping
 * `activeHost`. Best-effort + Tauri-only: resolves (never rejects) in the
 * vitest/web env, and swallows IPC failures so a flip is never blocked by
 * a proxy hiccup.
 */
function applyActiveDaemon(active: ActiveHost): Promise<void> {
  let base: string | null = null
  let token: string | null = null
  if (active !== 'local' && active.token && active.token.length > 0) {
    base = hostBaseUrl(active)
    token = active.token
  }
  // If a remote host has no usable token yet, fall through with
  // base/token = null → DaemonClient stays on local rather than firing
  // tokenless remote requests.
  //
  // `Promise.resolve(...)` wraps the invoke so a non-Tauri test double
  // that returns a bare value (not a promise) can't blow up the `.then`.
  return Promise.resolve(invoke('set_active_daemon', { base, token }))
    .then(() => undefined)
    .catch(() => {
      /* non-Tauri (vitest/web) or IPC failure — local stays the default */
    })
}

/** Result of {@link loginToHost}. On success the session token is also
 *  committed into the store (setHostToken) so daemon-ws.ts uses it. */
export type LoginResult =
  | { ok: true; token: string }
  | { ok: false; reason: string }

/** Daemon `POST /cli/auth/login` success body. */
interface LoginResponse {
  token: string
  username: string
  expiresAt: number
}

/**
 * Exchange `{username, password}` for a session token via
 * `POST <host>/cli/auth/login` (connect-users #617). On 200 the returned
 * session token is committed to the host (setHostToken) and the host's
 * lastConnectedAt is stamped, then cached to the keychain via
 * rememberToken so daemon-ws.ts / a fresh boot can reuse it. A 401 (or
 * any non-2xx) surfaces a friendly reason WITHOUT mutating state.
 *
 * The PASSWORD is NOT persisted here — the caller (Add-server /
 * RemoteSignIn) decides whether to rememberPassword based on the
 * "remember" toggle. This helper only deals in the session token.
 */
export async function loginToHost(
  host: ConnectHost,
  password: string,
  timeoutMs = 8000,
): Promise<LoginResult> {
  const username = host.username?.trim() ?? ''
  if (!username) {
    return { ok: false, reason: 'This server has no username configured.' }
  }
  const url = `${hostBaseUrl(host)}/cli/auth/login`
  let resp: Response
  try {
    resp = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
      signal: AbortSignal.timeout(timeoutMs),
    })
  } catch {
    return { ok: false, reason: `Couldn't reach ${host.hostname}. Check the address and your network.` }
  }
  if (resp.status === 401) {
    return { ok: false, reason: 'Invalid username or password.' }
  }
  if (!resp.ok) {
    return { ok: false, reason: `Server returned ${resp.status}. It may not be a K2 server.` }
  }
  let body: LoginResponse
  try {
    body = (await resp.json()) as LoginResponse
  } catch {
    return { ok: false, reason: 'Server response was not a valid login.' }
  }
  if (typeof body.token !== 'string' || body.token.length === 0) {
    return { ok: false, reason: 'Server did not return a session token.' }
  }
  // Commit the session token into the store + stamp lastConnectedAt so
  // daemon-ws.ts (which reads activeHost.token) picks it up, and cache it
  // to the keychain for a silent next connect.
  const store = useConnectHostStore.getState()
  store.setHostToken(host.id, body.token)
  const existing = store.hosts.find((h) => h.id === host.id)
  if (existing) {
    store.addHost({ ...existing, token: body.token, lastConnectedAt: Date.now() })
  }
  await rememberToken(host.id, body.token)
  return { ok: true, token: body.token }
}

export const useConnectHostStore = create<ConnectHostState>((set, get) => ({
  activeHost: 'local',
  hosts: loadHosts(),
  connectionStatus: 'connecting',
  pendingSignIn: null,

  selectHost: async (hostOrLocal) => {
    // Immediate UI feedback: enter 'connecting' + clear any pending
    // sign-in synchronously (we're committing to a host now). The gate
    // flips to 'connected' once the new host accepts. Crucially we do NOT
    // flip `activeHost` yet — that drives the `<App key={hostKey}>` remount
    // → fetchProjects(), which must not fire until the proxy override is in
    // place (see applyActiveDaemon).
    set({ connectionStatus: 'connecting', pendingSignIn: null })
    // Point the Tauri→daemon proxy layer at the new host and WAIT for it to
    // land, then flip activeHost. Awaiting closes the ordering race where
    // the remount's refetch could beat the override into place (a snap-back
    // to local would otherwise fetch the dead remote → 0 workspaces).
    await applyActiveDaemon(hostOrLocal)
    set({ activeHost: hostOrLocal })
  },

  requestSignIn: (host) => {
    set({ pendingSignIn: host })
  },

  expireSession: async (hostId) => {
    const { activeHost, hosts } = get()
    // Only act when the EXPIRED host is the active remote — a late 401
    // from a host we already switched away from must not hijack the UI.
    if (activeHost === 'local' || activeHost.id !== hostId) return
    // Drop the dead session token in memory + keychain (keep the
    // remembered password so RemoteSignIn can offer auto-login).
    const cleared = hosts.map((h) => (h.id === hostId ? { ...h, token: '' } : h))
    const clearedActive: ActiveHost = { ...activeHost, token: '' }
    void forgetToken(hostId)
    // Immediately reflect the dropped token in the host list + raise the
    // sign-in overlay (UI feedback) — these don't change `activeHost`'s
    // identity so they can't trigger a premature refetch.
    set({ hosts: cleared, pendingSignIn: clearedActive, connectionStatus: 'connecting' })
    // The session token is now empty → clear the proxy override back to
    // local BEFORE flipping `activeHost`, so the host-unaware commands the
    // remount re-fires don't keep hitting the dead remote.
    await applyActiveDaemon(clearedActive)
    set({ activeHost: clearedActive })
  },

  cancelSignIn: () => {
    set({ pendingSignIn: null })
  },

  pickHost: (hostOrLocal) => {
    if (hostOrLocal === 'local') {
      get().selectHost('local')
      return
    }
    // A usable in-memory session token (resolved from the keychain on
    // boot, or set during a prior login this session) → switch silently.
    if (hostOrLocal.token && hostOrLocal.token.length > 0) {
      get().selectHost(hostOrLocal)
      return
    }
    // connect-users (#617): no live session token, but if the user
    // remembered their PASSWORD we can auto-login without prompting.
    // Only applies to connect-users hosts (those have a username).
    if (hostOrLocal.username && hostOrLocal.username.length > 0) {
      void resolvePassword(hostOrLocal.id).then(async (pw) => {
        if (pw) {
          const result = await loginToHost(hostOrLocal, pw)
          if (result.ok) {
            // loginToHost committed the session token; re-read the host so
            // selectHost carries it, then switch silently.
            const refreshed = get().hosts.find((h) => h.id === hostOrLocal.id)
            get().selectHost(refreshed ?? { ...hostOrLocal, token: result.token })
            return
          }
        }
        // No remembered password, or it was rejected/expired → prompt.
        get().requestSignIn(hostOrLocal)
      })
      return
    }
    // Legacy raw-token host with no token → full-screen sign-in.
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
    // Drop any remembered session token AND password from the keychain
    // too (best-effort).
    void forgetToken(id)
    void forgetPassword(id)
    // If we removed the currently-active host, fall back to local.
    const nextActive: ActiveHost =
      activeHost !== 'local' && activeHost.id === id ? 'local' : activeHost
    set({ hosts: next, activeHost: nextActive })
  },

  setHostToken: async (id, token) => {
    const { hosts, activeHost } = get()
    const hosts2 = hosts.map((h) => (h.id === id ? { ...h, token } : h))
    // Update the host list immediately (no `activeHost`-identity change, so
    // no premature refetch).
    set({ hosts: hosts2 })
    // Keep the active host object in sync if it's the one being updated,
    // so daemon-ws.ts (which reads activeHost.token) sees the new token.
    const active2: ActiveHost =
      activeHost !== 'local' && activeHost.id === id
        ? { ...activeHost, token }
        : activeHost
    if (active2 === activeHost) return
    // We just set the token for the ACTIVE remote host (e.g. a silent
    // re-login committed a fresh session token). Refresh the proxy override
    // and WAIT for it to land before swapping the active host object, so the
    // host-unaware commands the remount re-fires carry the fresh token.
    await applyActiveDaemon(active2)
    set({ activeHost: active2 })
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

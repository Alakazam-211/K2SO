// 0.38.0 Commit 4 — daemon-authoritative session lifecycle subscription.
//
// Opens a long-lived WebSocket to `/cli/sessions/events?path=<workspace>`.
// The daemon pushes JSON frames every time a v2 session is registered or
// removed (and, eventually, renamed). Callers wire handlers per workspace
// to keep the tab store in sync with what the daemon thinks exists,
// replacing the old `sync:tabs` Tauri-event broadcast.
//
// Wire format (matches `crates/k2so-daemon/src/session_events.rs`):
//   { kind: 'hello',           workspace_path, subscriber_id }
//   { kind: 'session_added',   workspace_path, paneGroupId?, agent_name, command?, args, cwd?, isV2 }
//   { kind: 'session_removed', workspace_path, paneGroupId?, agent_name }
//   { kind: 'session_renamed', workspace_path, paneGroupId?, title }
//
// Backoff + auto-reconnect: a clean Close frame from the server is
// treated as "stop trying" (e.g. workspace deauthed); any other drop
// schedules a retry with exponential backoff (500ms → 5s cap). The
// caller's `onHello` handler fires every time the WS reconnects, so
// the tabs store can choose to re-reconcile after a transient drop.
//
// Cleanup: the returned `UnsubscribeFn` closes the socket, cancels
// any pending backoff timer, and prevents further reconnect attempts.

import { getDaemonWs, invalidateDaemonWs, daemonWsBase, type DaemonWsAvailable } from '@/kessel/daemon-ws'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useActiveStore } from '@/stores/active'
import { serverSupports } from '@/lib/server-capabilities'

// ── Wire types ───────────────────────────────────────────────────────────

export interface SessionAddedEvent {
  kind: 'session_added'
  workspace_path: string
  /** Serde renames `pane_group_id` → `paneGroupId` per the snake-vs-camel
   *  convention used elsewhere in the wire schema; we just match what the
   *  daemon emits. */
  pane_group_id: string | null
  agent_name: string
  command: string | null
  args: string[]
  session_id: string
  isV2: boolean
}

export interface SessionRemovedEvent {
  kind: 'session_removed'
  workspace_path: string
  pane_group_id: string | null
  agent_name: string
}

export interface SessionRenamedEvent {
  kind: 'session_renamed'
  workspace_path: string
  pane_group_id: string | null
  title: string
}

export interface HelloEvent {
  kind: 'hello'
  workspace_path: string
  subscriber_id: number
}

/**
 * Canonical daemon-owned Active delta (#672,
 * .k2so/prds/daemon-canonical-active.md §4.4). Broadcast on the SAME
 * session-events bus, but NOT tied to a workspace path — the daemon emits
 * it to every subscriber after any Active recompute (activate / pin /
 * dismiss / window-tick / reap-close). Carries the WHOLE set so client
 * convergence is trivial + order-independent (last-write-wins).
 */
export interface ActiveChangedEvent {
  kind: 'active_changed'
  activeProjectIds: string[]
  activeWindowHours: number
}

export type SessionEventMessage =
  | SessionAddedEvent
  | SessionRemovedEvent
  | SessionRenamedEvent
  | HelloEvent
  | ActiveChangedEvent

export interface SessionEventHandlers {
  onAdded?: (event: SessionAddedEvent) => void
  onRemoved?: (event: SessionRemovedEvent) => void
  onRenamed?: (event: SessionRenamedEvent) => void
  /** Fires after each successful (re)connect. Initial connect counts.
   *  Use it to trigger a one-shot reconcile so any events the renderer
   *  missed during the drop window get backfilled. */
  onHello?: (event: HelloEvent) => void
}

export type UnsubscribeFn = () => void

// ── Backoff config ───────────────────────────────────────────────────────

const INITIAL_BACKOFF_MS = 500
const MAX_BACKOFF_MS = 5_000

// ── Public API ───────────────────────────────────────────────────────────

/** Subscribe to daemon session lifecycle events for one workspace.
 *
 *  Returns an unsubscribe function. Calling it tears down the WS and
 *  stops the reconnect loop. Safe to call multiple times (idempotent).
 *
 *  The handler callbacks are invoked synchronously inside the WS
 *  `onmessage`/`onopen` handlers — keep them cheap, or marshal off to
 *  a setTimeout if they trigger heavy state churn. */
export function subscribeToWorkspaceSessionEvents(
  projectPath: string,
  handlers: SessionEventHandlers,
): UnsubscribeFn {
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    const delay = backoffMs
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  /**
   * Issue #5: idempotent reconnect trigger fired from BOTH `onerror`
   * AND `onclose`. The WHATWG spec sequences `onerror → onclose` for
   * an aborted connection, and the pre-0.39.8 code relied on that —
   * `onerror` was a no-op trusting `onclose` would follow. In
   * practice WebKit under process throttling (App Nap), network-
   * stack pressure, or WebKit-Networking-side hiccups can fire
   * `onerror` WITHOUT a follow-up `onclose`, which left the
   * subscriber permanently dead (no reconnect ever scheduled).
   *
   * Calling `triggerReconnect()` from both is safe because of the
   * `reconnectTimer !== null` early-out: the normal
   * `onerror → onclose` sequence schedules exactly one timer (the
   * second call is a no-op), and the pathological
   * `onerror-without-onclose` case still schedules one. The backoff
   * progression isn't double-advanced.
   */
  const triggerReconnect = (): void => {
    if (stopped) return
    if (reconnectTimer !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs()
    } catch (err) {
      // Daemon not reachable yet — invalidate the cached creds so the
      // retry pulls fresh values off disk, then schedule a backoff
      // attempt.
      invalidateDaemonWs()
      console.warn('[session-events] daemon credentials unavailable, retrying:', err)
      scheduleReconnect()
      return
    }

    if (stopped) return

    const url = `${daemonWsBase(creds)}/cli/sessions/events?path=${encodeURIComponent(projectPath)}&token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      ws = new WebSocket(url)
    } catch (err) {
      console.warn('[session-events] WS construction failed:', err)
      scheduleReconnect()
      return
    }
    socket = ws

    ws.onopen = () => {
      // Reset backoff so a long-lived stable connection doesn't pay
      // the previous failure's penalty on its next disconnect.
      backoffMs = INITIAL_BACKOFF_MS
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: SessionEventMessage
      try {
        msg = JSON.parse(raw) as SessionEventMessage
      } catch (err) {
        console.warn('[session-events] failed to parse frame:', err, raw)
        return
      }
      switch (msg.kind) {
        case 'hello':
          handlers.onHello?.(msg)
          break
        case 'session_added':
          handlers.onAdded?.(msg)
          break
        case 'session_removed':
          handlers.onRemoved?.(msg)
          break
        case 'session_renamed':
          handlers.onRenamed?.(msg)
          break
        case 'active_changed':
          // App-level concern (#672) — the per-workspace subscriber
          // ignores it; `subscribeToActiveState` consumes it. Swallow
          // here so it doesn't hit the unknown-kind warning.
          break
        default: {
          // Unknown kind — forward-compat, just log.
          const unknown = (msg as { kind?: string }).kind ?? 'unknown'
          console.warn('[session-events] unknown event kind:', unknown)
        }
      }
    }

    ws.onerror = () => {
      // 0.39.8 (Issue #5): always trigger reconnect on error. The
      // pre-0.39.8 assumption that `onclose` reliably follows
      // `onerror` doesn't hold under WebKit Networking throttling.
      // `triggerReconnect` is idempotent — if `onclose` does
      // follow, its trigger is a no-op (timer already pending).
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) {
        socket = null
      }
      if (stopped) return
      // A clean close (code 1000) from the server with no reason is
      // ambiguous — could be daemon shutdown, could be route gone.
      // Keep retrying; the daemon comes back fast enough that the
      // user won't notice, and a routed-away path would 403 on the
      // next attempt and the renderer just keeps trying (cheap).
      console.debug(
        `[session-events] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    stopped = true
    clearReconnect()
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}

// ── App-level Active-state mirror (#672) ──────────────────────────────────
//
// The daemon owns the canonical Active set and pushes `active_changed`
// deltas (the WHOLE set) on the SAME session-events bus. Unlike
// `subscribeToWorkspaceSessionEvents` (one WS per active workspace, cwd-
// filtered), this is ONE app-level WS opened at boot that mirrors the
// daemon's Active set 1:1 into `useActiveStore`:
//
//   - on (re)connect (`Hello`) → GET /cli/projects/active snapshot →
//     `setFromSnapshot` (corrects any drift after a transient drop), and
//   - on each `active_changed` frame → `applyActiveChanged` (full-set
//     replace, last-write-wins).
//
// It subscribes with an EMPTY workspace path so it isn't scoped to a
// single workspace; the daemon broadcasts `active_changed` to every
// subscriber regardless of the cwd filter (the event carries no cwd).
// Per-workspace session_added/removed frames that happen to arrive on
// this socket are ignored — the workspace subscriber owns those.
//
// Capability-gated: against a daemon WITHOUT `canonical-active` the
// snapshot route 404s and no deltas arrive, so this is a no-op mirror and
// the Active bar uses its local-derivation fallback. We still open the WS
// (cheap) so a host that gains the capability mid-session starts mirroring
// on its next reconnect snapshot.

/** Fetch the canonical Active snapshot and write it into useActiveStore.
 *  Host-aware (reads the active host via daemonCliGet). No-op when the
 *  active host doesn't advertise `canonical-active` (route absent). */
export async function refreshActiveSnapshot(): Promise<void> {
  if (!serverSupports('canonical-active')) return
  try {
    const snap = await daemonCliGet<{ projectIds: string[]; activeWindowHours: number }>(
      'projects/active',
    )
    useActiveStore.getState().setFromSnapshot({
      projectIds: Array.isArray(snap?.projectIds) ? snap.projectIds : [],
      activeWindowHours:
        typeof snap?.activeWindowHours === 'number' ? snap.activeWindowHours : 24,
    })
  } catch (err) {
    // Route absent (old daemon) or transient failure — leave the mirror
    // alone; the local fallback derivation covers display.
    console.debug('[active-state] snapshot fetch skipped:', err)
  }
}

/**
 * Open the single app-level Active-state subscription. Call once at app
 * boot. Returns an UnsubscribeFn that tears down the WS + reconnect loop.
 * On a host switch, tear this down and call it again (see connect-host
 * wiring) so a remote host's Active set mirrors 1:1.
 */
export function subscribeToActiveState(): UnsubscribeFn {
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    const delay = backoffMs
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  const triggerReconnect = (): void => {
    if (stopped) return
    if (reconnectTimer !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs()
    } catch (err) {
      invalidateDaemonWs()
      console.warn('[active-state] daemon credentials unavailable, retrying:', err)
      scheduleReconnect()
      return
    }
    if (stopped) return

    // Empty path → app-level subscriber (not scoped to one workspace).
    const url = `${daemonWsBase(creds)}/cli/sessions/events?path=&token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      ws = new WebSocket(url)
    } catch (err) {
      console.warn('[active-state] WS construction failed:', err)
      scheduleReconnect()
      return
    }
    socket = ws

    ws.onopen = () => {
      backoffMs = INITIAL_BACKOFF_MS
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: SessionEventMessage
      try {
        msg = JSON.parse(raw) as SessionEventMessage
      } catch (err) {
        console.warn('[active-state] failed to parse frame:', err, raw)
        return
      }
      if (msg.kind === 'hello') {
        // (Re)connected — pull a fresh snapshot to correct any drift
        // (deltas may have been missed during a drop window).
        void refreshActiveSnapshot()
        return
      }
      if (msg.kind === 'active_changed') {
        useActiveStore.getState().applyActiveChanged({
          activeProjectIds: Array.isArray(msg.activeProjectIds) ? msg.activeProjectIds : [],
          activeWindowHours:
            typeof msg.activeWindowHours === 'number' ? msg.activeWindowHours : 24,
        })
        return
      }
      // session_added / session_removed / session_renamed — owned by the
      // per-workspace subscriber; ignore on the app-level socket.
    }

    ws.onerror = () => {
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) socket = null
      if (stopped) return
      console.debug(
        `[active-state] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    stopped = true
    clearReconnect()
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}

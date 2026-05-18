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

import { getDaemonWs, invalidateDaemonWs } from '@/kessel/daemon-ws'

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

export type SessionEventMessage =
  | SessionAddedEvent
  | SessionRemovedEvent
  | SessionRenamedEvent
  | HelloEvent

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

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: { port: number; token: string }
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

    const url = `ws://127.0.0.1:${creds.port}/cli/sessions/events?path=${encodeURIComponent(projectPath)}&token=${encodeURIComponent(creds.token)}`
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
        default: {
          // Unknown kind — forward-compat, just log.
          const unknown = (msg as { kind?: string }).kind ?? 'unknown'
          console.warn('[session-events] unknown event kind:', unknown)
        }
      }
    }

    ws.onerror = () => {
      // onclose will follow with a non-1000 code; do nothing here so
      // we don't double-schedule the reconnect.
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
      scheduleReconnect()
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

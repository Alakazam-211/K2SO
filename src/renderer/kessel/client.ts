// Kessel — Session Stream WebSocket client.
//
// Opens `ws://<host>:<port>/cli/sessions/subscribe?session=<uuid>
// &token=<token>`, parses the daemon's envelope format into typed
// events, and fans them out to subscriber callbacks. Reconnects with
// exponential backoff on drop. Dispose cleans up everything.
//
// Lifecycle:
//   const client = new KesselClient({ sessionId, port, token })
//   const off = client.on({
//     onAck:    (a) => ...,
//     onFrame:  (f) => ...,
//     onError:  (e) => ...,
//     onOpen:   () => ...,
//     onClose:  (code) => ...,
//   })
//   client.connect()
//   // ...
//   off()           // unsubscribe but leave WS open
//   client.dispose() // close WS + cleanup (idempotent)

import type {
  AckPayload,
  ErrorPayload,
  Frame,
  KesselEnvelope,
} from './types'

export interface KesselClientOpts {
  /** SessionId UUID string. */
  sessionId: string
  /** Daemon port — from `invoke('daemon_ws_url')`. */
  port: number
  /** Auth token — from `invoke('daemon_ws_url')`. */
  token: string
  /** Override for `127.0.0.1`. Default is loopback. */
  host?: string
  /** Max reconnect attempts (default 5). Set to 0 to disable reconnect. */
  maxReconnectAttempts?: number
  /** Backoff base in ms (default 500). Doubles each attempt, capped at 10s. */
  reconnectBaseMs?: number
  /** D4: batch frames by animation frame instead of dispatching each
   *  as-it-arrives. Claude's bottom-border repaints emit ~100 frames
   *  per burst; batching cuts the per-frame callback overhead to one
   *  invocation per rAF. Enabled by default for callers that pass
   *  true; off by default for tests + simple consumers.
   *
   *  When on, listeners receive frames via `onFrames(batch)` if they
   *  define it; listeners that only define `onFrame` still get each
   *  frame sequentially inside the batch callback. Ordering is
   *  preserved (4.7 C4). */
  frameBatchingEnabled?: boolean
}

export interface KesselListener {
  onAck?: (ack: AckPayload) => void
  /** Per-frame callback. Fires once per WS frame arrival unless
   *  `frameBatchingEnabled` is true and `onFrames` is also defined,
   *  in which case `onFrames` is preferred and this one is skipped. */
  onFrame?: (frame: Frame) => void
  /** Batched frames callback — fires once per rAF with every frame
   *  that arrived since the last flush. Batching reduces React
   *  setState cost on heavy bursts (Claude repaints, htop at 1Hz).
   *  Requires opts.frameBatchingEnabled=true on the client. */
  onFrames?: (frames: readonly Frame[]) => void
  onError?: (err: ErrorPayload) => void
  onOpen?: () => void
  /** Fires on close. `code` is the WS close code. `willReconnect` tells
   *  listeners whether the client is about to retry. */
  onClose?: (code: number, willReconnect: boolean) => void
}

/** WebSocket factory — swapped out in tests. */
export type WsFactory = (url: string) => WebSocket

export class KesselClient {
  private readonly opts: Required<
    Omit<KesselClientOpts, 'host'>
  > & { host: string }
  private readonly wsFactory: WsFactory
  private readonly listeners = new Set<KesselListener>()
  private ws: WebSocket | null = null
  private disposed = false
  private reconnectAttempts = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  /** D4 batch buffer. Populated as frames arrive on the WS; drained
   *  by a single rAF callback. Only used when frameBatchingEnabled
   *  is true. */
  private pendingFrames: Frame[] = []
  private rafFlushPending = false

  constructor(opts: KesselClientOpts, wsFactory: WsFactory = (u) => new WebSocket(u)) {
    this.opts = {
      host: '127.0.0.1',
      maxReconnectAttempts: 5,
      reconnectBaseMs: 500,
      frameBatchingEnabled: false,
      ...opts,
    }
    this.wsFactory = wsFactory
  }

  /** Compose the full WS URL including auth + session params. Exposed
   *  so tests and diagnostic tooling can inspect the target without
   *  opening a real socket. */
  url(): string {
    const { host, port, sessionId, token } = this.opts
    // Token is 32 hex chars (daemon writes with generate_token); no
    // URL-encoding needed. SessionId is a UUID; same.
    return `ws://${host}:${port}/cli/sessions/subscribe?session=${sessionId}&token=${token}`
  }

  /** Add a listener. Returns an unsubscribe function. */
  on(listener: KesselListener): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  /** Open the WS and start streaming. Idempotent — repeated calls with
   *  an open socket are no-ops. */
  connect(): void {
    if (this.disposed) return
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) return
    this.open()
  }

  /** Close the WS and forbid reconnect. Cancels any pending retry.
   *  Idempotent — safe to call multiple times. */
  dispose(): void {
    this.disposed = true
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    if (this.ws) {
      // ws.close throws in some browsers if called pre-open; swallow.
      try {
        this.ws.close(1000, 'disposed')
      } catch {
        /* no-op */
      }
      this.ws = null
    }
    // Drop any buffered batch — listeners are being cleared too so
    // there's no one to deliver to. The pending rAF callback no-ops
    // because rafFlushPending is reset and pendingFrames is empty.
    this.pendingFrames = []
    this.rafFlushPending = false
    this.listeners.clear()
  }

  // ── Internals ────────────────────────────────────────────────────

  private open(): void {
    const ws = this.wsFactory(this.url())
    this.ws = ws

    ws.addEventListener('open', () => {
      this.reconnectAttempts = 0
      for (const l of this.listeners) l.onOpen?.()
    })

    ws.addEventListener('message', (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (!raw) return
      this.dispatch(raw)
    })

    ws.addEventListener('close', (ev) => {
      const code = (ev as CloseEvent).code ?? 1006
      this.ws = null
      if (this.disposed) {
        for (const l of this.listeners) l.onClose?.(code, false)
        return
      }
      const shouldReconnect =
        this.opts.maxReconnectAttempts > 0 &&
        this.reconnectAttempts < this.opts.maxReconnectAttempts
      for (const l of this.listeners) l.onClose?.(code, shouldReconnect)
      if (shouldReconnect) this.scheduleReconnect()
    })

    ws.addEventListener('error', () => {
      // `error` always precedes `close` on browser WS. We handle
      // reconnect in the close handler so this is just a diagnostic
      // hook for listeners that want to know the connection is
      // unhealthy before it formally closes.
    })
  }

  private dispatch(raw: string): void {
    let env: KesselEnvelope
    try {
      env = JSON.parse(raw) as KesselEnvelope
    } catch {
      // Daemon only ever sends valid JSON; a parse failure means
      // something is wrong with the wire. Surface as a synthesized
      // error so UIs can react.
      for (const l of this.listeners) {
        l.onError?.({ message: `invalid JSON envelope: ${raw.slice(0, 128)}` })
      }
      return
    }

    switch (env.event) {
      case 'session:ack':
        for (const l of this.listeners) l.onAck?.(env.payload)
        break
      case 'session:frame':
        this.handleFrame(env.payload)
        break
      case 'session:error':
        for (const l of this.listeners) l.onError?.(env.payload)
        break
      default: {
        // Future event types from the daemon land here. Log but
        // don't throw — forward-compat is a feature.
        const _exhaustive: never = env
        void _exhaustive
        break
      }
    }
  }

  private handleFrame(frame: Frame): void {
    if (!this.opts.frameBatchingEnabled) {
      this.dispatchFrameBatch([frame])
      return
    }
    this.pendingFrames.push(frame)
    if (this.rafFlushPending) return
    this.rafFlushPending = true
    const flush = (): void => {
      this.rafFlushPending = false
      const batch = this.pendingFrames
      if (batch.length === 0) return
      this.pendingFrames = []
      this.dispatchFrameBatch(batch)
    }
    // rAF is available in every DOM renderer environment that
    // matters; fall back to setTimeout for tests or headless cases.
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(flush)
    } else {
      setTimeout(flush, 0)
    }
  }

  /** Dispatch a frame batch to all listeners. Listeners that defined
   *  `onFrames` receive the full batch as a readonly array. Listeners
   *  that only defined `onFrame` receive each frame sequentially,
   *  preserving arrival order (4.7 C4). */
  private dispatchFrameBatch(batch: readonly Frame[]): void {
    for (const l of this.listeners) {
      if (l.onFrames) {
        l.onFrames(batch)
      } else if (l.onFrame) {
        for (const f of batch) l.onFrame(f)
      }
    }
  }

  private scheduleReconnect(): void {
    this.reconnectAttempts += 1
    const delayMs = Math.min(
      this.opts.reconnectBaseMs * 2 ** (this.reconnectAttempts - 1),
      10_000,
    )
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.disposed) this.open()
    }, delayMs)
  }
}

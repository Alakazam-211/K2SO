// Unit tests for KesselClient. Mocks WebSocket with a minimal fake
// that exposes `fire*` helpers so tests can drive the socket from
// the outside without spinning up a real WS server.
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { KesselClient, type KesselListener } from './client'
import type { Frame } from './types'

// ── Fake WebSocket ─────────────────────────────────────────────────

interface FakeListenerMap {
  open: Array<(ev: Event) => void>
  message: Array<(ev: MessageEvent) => void>
  close: Array<(ev: CloseEvent) => void>
  error: Array<(ev: Event) => void>
}

class FakeWebSocket {
  static OPEN = 1
  readyState = 0
  url: string
  private ls: FakeListenerMap = { open: [], message: [], close: [], error: [] }
  constructor(url: string) {
    this.url = url
  }
  addEventListener<K extends keyof FakeListenerMap>(
    kind: K,
    cb: FakeListenerMap[K][number],
  ) {
    ;(this.ls[kind] as Array<unknown>).push(cb)
  }
  close(code?: number) {
    this.readyState = 3
    this.fireClose(code ?? 1000)
  }
  // Test helpers — not in the real WS API.
  fireOpen() {
    this.readyState = 1
    for (const cb of this.ls.open) cb(new Event('open'))
  }
  fireMessage(data: string) {
    for (const cb of this.ls.message) {
      cb({ data } as MessageEvent)
    }
  }
  fireClose(code = 1000) {
    this.readyState = 3
    for (const cb of this.ls.close) {
      cb({ code } as CloseEvent)
    }
  }
}

// Provide static members the real WebSocket has that the client
// references (WebSocket.OPEN). vitest's jsdom-less `node` env has
// no WebSocket global; stub it on globalThis once per test file.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
;(globalThis as any).WebSocket = FakeWebSocket

// ── Helpers ────────────────────────────────────────────────────────

function makeClient() {
  let lastWs: FakeWebSocket | null = null
  const factory = (url: string) => {
    lastWs = new FakeWebSocket(url)
    return lastWs as unknown as WebSocket
  }
  const client = new KesselClient(
    {
      sessionId: '550e8400-e29b-41d4-a716-446655440000',
      port: 12345,
      token: 'deadbeefcafebabe',
      reconnectBaseMs: 1, // fast retries for tests
      maxReconnectAttempts: 2,
    },
    factory,
  )
  return {
    client,
    getWs: () => lastWs!,
  }
}

// ── Tests ──────────────────────────────────────────────────────────

describe('KesselClient.url', () => {
  it('composes the daemon WS URL with session + token params', () => {
    const { client } = makeClient()
    expect(client.url()).toBe(
      'ws://127.0.0.1:12345/cli/sessions/subscribe?session=550e8400-e29b-41d4-a716-446655440000&token=deadbeefcafebabe',
    )
  })

  it('honors a custom host', () => {
    const c = new KesselClient({
      sessionId: 'abc',
      port: 9000,
      token: 't',
      host: 'tailscale.host',
    })
    expect(c.url()).toBe(
      'ws://tailscale.host:9000/cli/sessions/subscribe?session=abc&token=t',
    )
  })
})

describe('KesselClient envelope dispatch', () => {
  let events: {
    opens: number
    acks: Array<{ sessionId: string; replayCount: number }>
    frames: Frame[]
    errors: string[]
    closes: Array<[number, boolean]>
  }
  let listener: KesselListener

  beforeEach(() => {
    events = { opens: 0, acks: [], frames: [], errors: [], closes: [] }
    listener = {
      onOpen: () => events.opens++,
      onAck: (a) => events.acks.push(a),
      onFrame: (f) => events.frames.push(f),
      onError: (e) => events.errors.push(e.message),
      onClose: (code, retry) => events.closes.push([code, retry]),
    }
  })

  it('dispatches session:ack to onAck', () => {
    const { client, getWs } = makeClient()
    client.on(listener)
    client.connect()
    getWs().fireOpen()
    getWs().fireMessage(
      JSON.stringify({
        event: 'session:ack',
        payload: { sessionId: 'abc', replayCount: 3 },
      }),
    )
    expect(events.acks).toEqual([{ sessionId: 'abc', replayCount: 3 }])
    expect(events.opens).toBe(1)
  })

  it('dispatches session:frame text payloads with raw Vec<u8> shape', () => {
    const { client, getWs } = makeClient()
    client.on(listener)
    client.connect()
    getWs().fireOpen()
    // Bytes = "hi" (0x68 0x69).
    const textFrame = {
      event: 'session:frame',
      payload: {
        frame: 'Text',
        data: { bytes: [0x68, 0x69], style: null },
      },
    }
    getWs().fireMessage(JSON.stringify(textFrame))

    expect(events.frames.length).toBe(1)
    const f = events.frames[0]
    expect(f.frame).toBe('Text')
    if (f.frame === 'Text') {
      expect(f.data.bytes).toEqual([0x68, 0x69])
      expect(f.data.style).toBeNull()
    }
  })

  it('dispatches session:frame CursorOp variants adjacent-tagged', () => {
    const { client, getWs } = makeClient()
    client.on(listener)
    client.connect()
    getWs().fireOpen()
    getWs().fireMessage(
      JSON.stringify({
        event: 'session:frame',
        payload: {
          frame: 'CursorOp',
          data: { op: 'Goto', value: { row: 5, col: 10 } },
        },
      }),
    )
    const f = events.frames[0]
    expect(f.frame).toBe('CursorOp')
    if (f.frame === 'CursorOp') {
      expect(f.data.op).toBe('Goto')
      if (f.data.op === 'Goto') {
        expect(f.data.value).toEqual({ row: 5, col: 10 })
      }
    }
  })

  it('surfaces invalid JSON as a synthesized error', () => {
    const { client, getWs } = makeClient()
    client.on(listener)
    client.connect()
    getWs().fireOpen()
    getWs().fireMessage('{ not json')
    expect(events.errors.length).toBe(1)
    expect(events.errors[0]).toMatch(/invalid JSON/)
  })

  it('routes session:error to onError', () => {
    const { client, getWs } = makeClient()
    client.on(listener)
    client.connect()
    getWs().fireOpen()
    getWs().fireMessage(
      JSON.stringify({
        event: 'session:error',
        payload: { message: 'session not found' },
      }),
    )
    expect(events.errors).toEqual(['session not found'])
  })
})

describe('KesselClient lifecycle', () => {
  it('returns a detach fn from on() that removes the listener', () => {
    const { client, getWs } = makeClient()
    let calls = 0
    const off = client.on({ onFrame: () => calls++ })
    client.connect()
    getWs().fireOpen()
    getWs().fireMessage(
      JSON.stringify({
        event: 'session:frame',
        payload: { frame: 'Text', data: { bytes: [0x61], style: null } },
      }),
    )
    expect(calls).toBe(1)
    off()
    getWs().fireMessage(
      JSON.stringify({
        event: 'session:frame',
        payload: { frame: 'Text', data: { bytes: [0x62], style: null } },
      }),
    )
    expect(calls).toBe(1) // unchanged after detach
  })

  it('reports willReconnect=true on close when attempts remain', () => {
    const { client, getWs } = makeClient()
    const closes: Array<[number, boolean]> = []
    client.on({ onClose: (code, retry) => closes.push([code, retry]) })
    client.connect()
    getWs().fireOpen()
    getWs().fireClose(1006)
    expect(closes[0]).toEqual([1006, true])
  })

  it('does NOT reconnect after dispose()', async () => {
    vi.useFakeTimers()
    const { client, getWs } = makeClient()
    const closes: Array<[number, boolean]> = []
    client.on({ onClose: (code, retry) => closes.push([code, retry]) })
    client.connect()
    getWs().fireOpen()
    client.dispose()
    // dispose() closes the socket; the close handler sees disposed=true
    // and reports willReconnect=false.
    expect(closes[0]).toEqual([1000, false])
    vi.useRealTimers()
  })

  it('is idempotent across connect/dispose cycles', () => {
    const { client } = makeClient()
    client.connect()
    client.connect() // no-op
    client.dispose()
    client.dispose() // no-op, no throw
  })
})

// @vitest-environment jsdom
// Unit tests for SessionStreamView. Mocks WebSocket + rAF so we can
// drive frames from the outside and assert on rendered DOM.
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, cleanup, act } from '@testing-library/react'
import { SessionStreamView } from './SessionStreamView'

// ── Fake WebSocket (same pattern as client.test.ts) ────────────────

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
    FakeWebSocket.last = this
  }
  static last: FakeWebSocket | null = null
  addEventListener<K extends keyof FakeListenerMap>(
    kind: K,
    cb: FakeListenerMap[K][number],
  ) {
    ;(this.ls[kind] as Array<unknown>).push(cb)
  }
  close() {
    this.readyState = 3
    for (const cb of this.ls.close) cb({ code: 1000 } as CloseEvent)
  }
  fireOpen() {
    this.readyState = 1
    for (const cb of this.ls.open) cb(new Event('open'))
  }
  fireMessage(data: string) {
    for (const cb of this.ls.message) {
      cb({ data } as MessageEvent)
    }
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
;(globalThis as any).WebSocket = FakeWebSocket

// Flush rAF synchronously so each `fireMessage` reflects to DOM in
// the same tick. Keeps tests sync-clean with no timers.
beforeEach(() => {
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    cb(0)
    return 0 as unknown as number
  })
  FakeWebSocket.last = null
})

function textFrameEnvelope(s: string): string {
  const bytes = Array.from(new TextEncoder().encode(s))
  return JSON.stringify({
    event: 'session:frame',
    payload: { frame: 'Text', data: { bytes, style: null } },
  })
}

// ── Tests ──────────────────────────────────────────────────────────

describe('SessionStreamView', () => {
  it('renders an empty grid on first mount', () => {
    const { container } = render(
      <SessionStreamView
        sessionId="abc"
        port={9000}
        token="t"
        cols={5}
        rows={2}
      />,
    )
    const rows = container.querySelectorAll('.kessel-session-stream-view > div')
    // 2 grid rows + 1 cursor overlay.
    expect(rows.length).toBe(3)
    cleanup()
  })

  it('renders text from incoming Frame::Text events', () => {
    const { container } = render(
      <SessionStreamView
        sessionId="abc"
        port={9000}
        token="t"
        cols={10}
        rows={3}
      />,
    )
    act(() => {
      FakeWebSocket.last!.fireOpen()
      FakeWebSocket.last!.fireMessage(textFrameEnvelope('hello'))
    })

    // Row 0 should contain "hello" (padded with spaces).
    const rowDivs = container.querySelectorAll(
      '.kessel-session-stream-view > div',
    )
    // First two are grid rows; third is cursor.
    const row0 = rowDivs[0]
    expect(row0.textContent?.startsWith('hello')).toBe(true)
    cleanup()
  })

  it('calls onReady when session:ack arrives', () => {
    const onReady = vi.fn()
    render(
      <SessionStreamView
        sessionId="abc"
        port={9000}
        token="t"
        cols={5}
        rows={2}
        onReady={onReady}
      />,
    )
    act(() => {
      FakeWebSocket.last!.fireOpen()
      FakeWebSocket.last!.fireMessage(
        JSON.stringify({
          event: 'session:ack',
          payload: { sessionId: 'abc', replayCount: 7 },
        }),
      )
    })
    expect(onReady).toHaveBeenCalledWith(7)
    cleanup()
  })

  it('calls onError on invalid JSON', () => {
    const onError = vi.fn()
    render(
      <SessionStreamView
        sessionId="abc"
        port={9000}
        token="t"
        cols={5}
        rows={2}
        onError={onError}
      />,
    )
    act(() => {
      FakeWebSocket.last!.fireOpen()
      FakeWebSocket.last!.fireMessage('not json')
    })
    expect(onError).toHaveBeenCalled()
    expect(onError.mock.calls[0][0]).toMatch(/invalid JSON/)
    cleanup()
  })
})

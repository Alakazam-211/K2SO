// 0.39.39 (#675/#677) — renderer cutover from polling to daemon broadcasts.
//
// Each renderer consumer (assistant / active-agents / review-queue /
// ReviewPanel / CompanionSection) drops its `setInterval`/`setTimeout` poll
// and subscribes to the Wave B broadcast when the daemon supports it
// (`serverSupports('daemon-broadcasts')`), and KEEPS its polling fallback
// when it does not. This suite proves, per consumer:
//   - SUPPORTED  → it subscribes to its event AND does not install its poll.
//   - SUPPORTED  → its event handler triggers a refetch.
//   - UNSUPPORTED → it installs its poll (fallback) and does NOT subscribe.
//
// vitest env is `node` (no Tauri / WebSocket). We mock the session-events
// subscription helpers (the boundary this wave owns) + serverSupports, so
// importing the consumers is inert and we assert the wiring directly.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the consumers import) ───────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

// The capability gate — flipped per test.
const supports = vi.hoisted(() => ({ value: true }))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => supports.value),
}))

// The session-events helpers this wave owns. We record registered handlers
// so a test can fire the event the consumer subscribed to.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  const reg = {
    llm: [] as Fn[],
    agent: [] as Fn[],
    tunnel: [] as Fn[],
    appHello: [] as Fn[],
    reviewSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
  }
  return { reg }
})
vi.mock('@/stores/session-events', () => ({
  onLlmStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.llm.push(fn)
    return () => void (ev.reg.llm = ev.reg.llm.filter((f) => f !== fn))
  }),
  onAgentStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.agent.push(fn)
    return () => void (ev.reg.agent = ev.reg.agent.filter((f) => f !== fn))
  }),
  onTunnelStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.tunnel.push(fn)
    return () => void (ev.reg.tunnel = ev.reg.tunnel.filter((f) => f !== fn))
  }),
  onAppHello: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.appHello.push(fn)
    return () => void (ev.reg.appHello = ev.reg.appHello.filter((f) => f !== fn))
  }),
  subscribeToWorkspaceReviewEvents: vi.fn(
    (path: string, handlers: Record<string, (...a: unknown[]) => void>) => {
      const entry = { path, handlers }
      ev.reg.reviewSubs.push(entry)
      return () => void (ev.reg.reviewSubs = ev.reg.reviewSubs.filter((e) => e !== entry))
    },
  ),
}))

// Daemon transport boundary — keep every fire-and-forget fetch inert so a
// store's module-load / startup side effects (settings fetch, agents/running
// snapshot) don't reach the network in node and reject.
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 0, token: 't', secure: false, host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:0'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:0'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalCreate: vi.fn(async () => undefined),
  terminalExists: vi.fn(async () => false),
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: Object.assign(vi.fn(() => undefined), {
    getState: () => ({ defaultAgent: null, agenticSystemsEnabled: true, fetchSettings: vi.fn() }),
    setState: vi.fn(),
    subscribe: vi.fn(() => () => undefined),
  }),
}))

// `startAgentPolling` fire-and-forgets `import('@tauri-apps/api/event')` to
// wire legacy hook/CLI listeners. The dynamic import resolves the REAL Tauri
// transport, which reaches for `window` and throws in node. That wiring is
// orthogonal to the broadcast cutover under test; stub a minimal `window` so
// `listen(...)` is inert instead of producing unhandled rejections.
vi.stubGlobal('window', {
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  __TAURI_INTERNALS__: { transformCallback: () => 0, invoke: async () => undefined },
})

// localStorage stub (some stores load it transitively in node).
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

beforeEach(() => {
  supports.value = true
  ev.reg.llm = []
  ev.reg.agent = []
  ev.reg.tunnel = []
  ev.reg.appHello = []
  ev.reg.reviewSubs = []
  vi.clearAllTimers()
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
})

// ── active-agents (#675.2) ──────────────────────────────────────────────

describe('active-agents — agent_status_changed cutover', () => {
  it('subscribes (no interval) when supported and refetches via lifecycle handler', async () => {
    vi.useFakeTimers()
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    const handle = vi
      .spyOn(useActiveAgentsStore.getState(), 'handleLifecycleEvent')
      .mockImplementation(() => undefined)
    // Also stub pollOnce so the initial snapshot doesn't hit the daemon.
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()
    expect(ev.reg.agent.length).toBe(1)
    expect(setInterval).not.toHaveBeenCalled()

    // Firing the subscribed event maps through the canonical lifecycle path.
    ev.reg.agent[0]({ paneId: 'p1', tabId: 't1', status: 'start' })
    expect(handle).toHaveBeenCalledWith('p1', 't1', 'start')

    stopAgentPolling()
  })

  it('falls back to the poll interval when NOT supported (no subscription)', async () => {
    vi.useFakeTimers()
    supports.value = false
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()
    expect(ev.reg.agent.length).toBe(0)
    expect(setInterval).toHaveBeenCalledTimes(1)

    stopAgentPolling()
  })
})

// ── review-queue (#675.3) ───────────────────────────────────────────────

describe('review-queue — review_changed cutover', () => {
  it('subscribes (no interval) when supported and refetches on event', async () => {
    vi.useFakeTimers()
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startReviewQueuePolling, stopReviewQueuePolling, useReviewQueueStore } =
      await import('./review-queue')
    const { useProjectsStore } = await import('./projects')
    // Give the store an active project so it subscribes on a real path.
    useProjectsStore.setState({
      activeProjectId: 'pr1',
      projects: [{ id: 'pr1', path: '/ws/foo', workspaces: [] }] as never,
    })
    const fetchAll = vi
      .spyOn(useReviewQueueStore.getState(), 'fetchAll')
      .mockResolvedValue(undefined)

    startReviewQueuePolling()
    expect(setInterval).not.toHaveBeenCalled()
    expect(ev.reg.reviewSubs.length).toBe(1)
    expect(ev.reg.reviewSubs[0].path).toBe('/ws/foo')

    const callsBefore = fetchAll.mock.calls.length
    ev.reg.reviewSubs[0].handlers.onReviewChanged({
      kind: 'review_changed',
      workspacePath: '/ws/foo',
      agent: null,
    })
    expect(fetchAll.mock.calls.length).toBe(callsBefore + 1)

    stopReviewQueuePolling()
  })

  it('falls back to the 30s poll when NOT supported (no subscription)', async () => {
    vi.useFakeTimers()
    supports.value = false
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startReviewQueuePolling, stopReviewQueuePolling, useReviewQueueStore } =
      await import('./review-queue')
    vi.spyOn(useReviewQueueStore.getState(), 'fetchAll').mockResolvedValue(undefined)

    startReviewQueuePolling()
    expect(ev.reg.reviewSubs.length).toBe(0)
    expect(setInterval).toHaveBeenCalledTimes(1)
    expect(setInterval).toHaveBeenCalledWith(expect.any(Function), 30_000)

    stopReviewQueuePolling()
  })
})

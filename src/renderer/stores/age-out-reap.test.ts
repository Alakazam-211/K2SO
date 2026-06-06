// P2 — age-out sweep. A workspace whose `lastInteractionAt` has aged
// past the configured Active window has its background pinned-Chat PTY
// reaped via the SAME #657 dismiss path (15s grace + cancel-on-return
// + fire-time foreground re-check). "Aged out of Active" therefore
// behaves exactly like an implicit dismiss.
//
// This suite drives `sweepAgedOutWorkspaceChats` against the REAL tabs
// store with fake timers, mocking the daemon/Tauri boundaries the tabs
// module touches (same boundary set as dismiss-reap.test.ts) so the
// decision logic is exercised without a live daemon.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────

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
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))

const closedAgentNames: string[] = []
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))

const setChatSessionCalls: Array<{ projectPath: string; sessionId: string }> = []
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
  setChatSession: vi.fn(async (projectPath: string, sessionId: string) => {
    setChatSessionCalls.push({ projectPath, sessionId })
  }),
}))

// Now import the REAL tabs store under test.
import {
  useTabsStore,
  registerActiveProjectIdGetter,
  DISMISS_REAP_GRACE_MS,
  __clearPendingChatReapsForTests,
  __hasPendingChatReapForTests,
  type AgeOutSweepCandidate,
} from './tabs'

// ── Test harness ─────────────────────────────────────────────────────────

let foregroundProjectId: string | null = null
registerActiveProjectIdGetter(() => foregroundProjectId)

const fetchSpy = vi.fn(async (_url: string, init?: { body?: string }) => {
  if (init?.body) {
    try {
      const parsed = JSON.parse(init.body)
      if (parsed.agent_name) closedAgentNames.push(parsed.agent_name)
    } catch { /* ignore */ }
  }
  return { ok: true, status: 200, text: async () => 'ok' } as unknown as Response
})

// Seed a background-workspace snapshot whose pinned Chat agent item
// carries a persisted Claude session id — the live PTY the sweep reaps.
const CHAT_SESSION_ID = 'claude-session-uuid-ageout'
function seedBackgroundChat(projectId: string, projectPath: string): void {
  const prev = useTabsStore.getState().backgroundWorkspaces
  useTabsStore.setState({
    backgroundWorkspaces: {
      ...prev,
      [`${projectId}:ws1`]: {
        tabs: [
          {
            id: `tab-chat-${projectId}`,
            paneGroups: new Map([
              ['pg-1', {
                items: [
                  {
                    type: 'agent',
                    data: {
                      agentName: 'k2so',
                      projectPath,
                      section: 'chat',
                      sessionId: CHAT_SESSION_ID,
                    },
                  },
                ],
                activeItemIndex: 0,
              }],
            ]),
          },
        ],
        extraGroups: [],
      } as never,
    },
  })
}

/** Default candidate = aged out, not pinned, no heartbeat. Override
 *  per-test to flip a single gate. */
function candidate(
  projectId: string,
  projectPath: string,
  overrides: Partial<AgeOutSweepCandidate> = {},
): AgeOutSweepCandidate {
  return {
    projectId,
    projectPath,
    isAged: true,
    manuallyActive: false,
    heartbeatEnabled: false,
    ...overrides,
  }
}

describe('#P2 age-out sweep (renderer)', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    foregroundProjectId = 'proj-FOREGROUND' // never one of the swept projects
    closedAgentNames.length = 0
    setChatSessionCalls.length = 0
    fetchSpy.mockClear()
    vi.stubGlobal('fetch', fetchSpy)
    __clearPendingChatReapsForTests()
    useTabsStore.setState({ backgroundWorkspaces: {} })
  })

  afterEach(() => {
    __clearPendingChatReapsForTests()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('reaps an aged-out, unpinned, non-foreground, no-heartbeat workspace after the grace', async () => {
    const projectId = 'proj-A'
    const projectPath = '/work/proj-A'
    seedBackgroundChat(projectId, projectPath)

    useTabsStore.getState().sweepAgedOutWorkspaceChats([candidate(projectId, projectPath)])
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    // Nothing fires before the grace window elapses.
    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS - 1)
    expect(closedAgentNames.length).toBe(0)

    vi.advanceTimersByTime(1)
    await vi.runAllTimersAsync()

    // Session id persisted (so --resume has a target) BEFORE the reap.
    expect(setChatSessionCalls).toEqual([
      { projectPath, sessionId: CHAT_SESSION_ID },
    ])
    expect(closedAgentNames).toContain(projectId)
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)
  })

  it('cancels the reap when the workspace re-activates within the grace', async () => {
    const projectId = 'proj-B'
    const projectPath = '/work/proj-B'
    seedBackgroundChat(projectId, projectPath)

    useTabsStore.getState().sweepAgedOutWorkspaceChats([candidate(projectId, projectPath)])
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    // Re-activation within the window cancels (the same call the
    // projects store makes from setActiveProject/setActiveWorkspace).
    vi.advanceTimersByTime(5_000)
    useTabsStore.getState().cancelWorkspaceChatReap(projectId)
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()
    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)
  })

  it('never age-out-reaps the foreground / manuallyActive / enabled-heartbeat workspaces', async () => {
    const fgPath = '/work/fg'
    const pinnedPath = '/work/pinned'
    const hbPath = '/work/hb'
    seedBackgroundChat('proj-FOREGROUND', fgPath)
    seedBackgroundChat('proj-PINNED', pinnedPath)
    seedBackgroundChat('proj-HB', hbPath)

    useTabsStore.getState().sweepAgedOutWorkspaceChats([
      // foreground (matches registered active project id) — aged but viewed.
      candidate('proj-FOREGROUND', fgPath),
      // pinned to Active.
      candidate('proj-PINNED', pinnedPath, { manuallyActive: true }),
      // enabled heartbeat — kept warm for autonomous wake.
      candidate('proj-HB', hbPath, { heartbeatEnabled: true }),
    ])

    expect(__hasPendingChatReapForTests('proj-FOREGROUND')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-PINNED')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-HB')).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS * 2)
    await vi.runAllTimersAsync()
    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)
  })

  it('does NOT reap a workspace still inside the Active window (isAged=false)', async () => {
    const projectId = 'proj-FRESH'
    const projectPath = '/work/proj-FRESH'
    seedBackgroundChat(projectId, projectPath)

    useTabsStore.getState().sweepAgedOutWorkspaceChats([
      candidate(projectId, projectPath, { isAged: false }),
    ])
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS * 2)
    await vi.runAllTimersAsync()
    expect(closedAgentNames.length).toBe(0)
  })

  it('skips a project with no background chat session (nothing to reap)', async () => {
    // No seedBackgroundChat — candidate is aged/unpinned/no-hb but has
    // no live pinned-Chat PTY, so the sweep must not schedule a no-op.
    useTabsStore.getState().sweepAgedOutWorkspaceChats([
      candidate('proj-NONE', '/work/proj-NONE'),
    ])
    expect(__hasPendingChatReapForTests('proj-NONE')).toBe(false)
  })
})

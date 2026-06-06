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
// The daemon-running sweep calls `daemonCliGet('terminal/list-running')`
// (via terminalListRunning). Route it to a per-test-controllable list so
// we can model "the daemon holds a live agent-chat:<projectId> PTY for a
// HIDDEN, aged-out workspace the renderer never opened".
let runningPtys: Array<{ terminalId: string; cwd: string; command: string | null }> = []
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    if (route === 'terminal/list-running') return runningPtys
    return []
  }),
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
  type AgeOutProjectMeta,
} from './tabs'
import { agentChatId } from '@/lib/terminal-id'

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
    runningPtys = []
    useTabsStore.setState({ backgroundWorkspaces: {}, tabs: [] })
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

// ── LIVE-fix — daemon-driven sweep reaches HIDDEN aged-out workspaces ─────
//
// The Active-bar-fed `sweepAgedOutWorkspaceChats` can only see candidates
// that are IN the Active bar — but an aged-out workspace is by definition
// NOT in the bar, so a workspace that ages out WHILE HIDDEN (or whose
// chat PTY survives from a prior app session, never opened in this
// renderer) was invisible to the sweep and never reaped. The LIVE smoke
// test caught exactly this: 6 aged-out workspaces with live daemon chat
// PTYs that survived multiple reaper ticks.
//
// `sweepAgedOutWorkspaceChatsFromDaemon` fixes it by enumerating the
// daemon's live `agent-chat:<projectId>` PTYs as the candidate source,
// independent of any renderer-side Active-bar / backgroundWorkspaces
// state, then applying the SAME age-out gates.

function meta(overrides: Partial<AgeOutProjectMeta> = {}): AgeOutProjectMeta {
  return {
    projectPath: '/work/x',
    isAged: true,
    manuallyActive: false,
    heartbeatEnabled: false,
    ...overrides,
  }
}

describe('#P2 age-out daemon sweep (renderer) — reaches hidden workspaces', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    foregroundProjectId = 'proj-FOREGROUND'
    closedAgentNames.length = 0
    setChatSessionCalls.length = 0
    fetchSpy.mockClear()
    vi.stubGlobal('fetch', fetchSpy)
    __clearPendingChatReapsForTests()
    runningPtys = []
    // Crucially: NO backgroundWorkspaces snapshot and NO live tabs — the
    // hidden workspace was never opened in this renderer session. The old
    // snapshot-fed sweep would find nothing to reap; the daemon list is
    // the only evidence the PTY is alive.
    useTabsStore.setState({ backgroundWorkspaces: {}, tabs: [] })
  })

  afterEach(() => {
    __clearPendingChatReapsForTests()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('reaps a HIDDEN, aged-out, non-pinned, non-heartbeat workspace with a live daemon chat PTY', async () => {
    const projectId = 'proj-HIDDEN'
    const projectPath = '/work/proj-HIDDEN'
    // The daemon holds a live chat PTY for this workspace, but the
    // renderer never opened it (no snapshot, no tab).
    runningPtys = [
      { terminalId: agentChatId(projectId, ''), cwd: projectPath, command: 'claude' },
    ]

    // Sanity: the snapshot-fed sweep can't see it (no renderer state).
    useTabsStore.getState().sweepAgedOutWorkspaceChats([
      candidate(projectId, projectPath),
    ])
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    // The daemon-fed sweep DOES see it (the LIVE-fix).
    await useTabsStore.getState().sweepAgedOutWorkspaceChatsFromDaemon({
      [projectId]: meta({ projectPath }),
    })
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()

    // The v2 session (agent_name === projectId) is closed even though no
    // renderer sessionId existed — the daemon already saved it for resume.
    expect(closedAgentNames).toContain(projectId)
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)
  })

  it('honors the keep-warm gates from the daemon list (foreground / pinned / heartbeat / fresh / no-verdict)', async () => {
    runningPtys = [
      { terminalId: agentChatId('proj-FOREGROUND', ''), cwd: '/work/fg', command: 'claude' },
      { terminalId: agentChatId('proj-PINNED', ''), cwd: '/work/pinned', command: 'claude' },
      { terminalId: agentChatId('proj-HB', ''), cwd: '/work/hb', command: 'claude' },
      { terminalId: agentChatId('proj-FRESH', ''), cwd: '/work/fresh', command: 'claude' },
      // Live PTY with NO verdict in the meta map — must be left alone.
      { terminalId: agentChatId('proj-UNKNOWN', ''), cwd: '/work/unknown', command: 'claude' },
      // A non-chat PTY — must be ignored entirely.
      { terminalId: 'plain-terminal-123', cwd: '/work/term', command: 'zsh' },
    ]

    await useTabsStore.getState().sweepAgedOutWorkspaceChatsFromDaemon({
      'proj-FOREGROUND': meta({ projectPath: '/work/fg' }),
      'proj-PINNED': meta({ projectPath: '/work/pinned', manuallyActive: true }),
      'proj-HB': meta({ projectPath: '/work/hb', heartbeatEnabled: true }),
      'proj-FRESH': meta({ projectPath: '/work/fresh', isAged: false }),
      // proj-UNKNOWN deliberately absent.
    })

    expect(__hasPendingChatReapForTests('proj-FOREGROUND')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-PINNED')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-HB')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-FRESH')).toBe(false)
    expect(__hasPendingChatReapForTests('proj-UNKNOWN')).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS * 2)
    await vi.runAllTimersAsync()
    expect(closedAgentNames.length).toBe(0)
  })

  it('cancel-on-return works for a daemon-swept reap (re-activation within the grace)', async () => {
    const projectId = 'proj-RETURN'
    const projectPath = '/work/proj-RETURN'
    runningPtys = [
      { terminalId: agentChatId(projectId, ''), cwd: projectPath, command: 'claude' },
    ]

    await useTabsStore.getState().sweepAgedOutWorkspaceChatsFromDaemon({
      [projectId]: meta({ projectPath }),
    })
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    vi.advanceTimersByTime(5_000)
    useTabsStore.getState().cancelWorkspaceChatReap(projectId)
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()
    expect(closedAgentNames.length).toBe(0)
  })
})

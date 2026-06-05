// #657 — dismissing a workspace from the Active bar reaps its PINNED
// Chat (agent) PTY after a 15s grace delay to free memory. On return
// the saved session lazily resumes via `claude --resume`.
//
// This suite drives the renderer half of the feature against the REAL
// tabs store with fake timers:
//
//   1. dismiss schedules a reap; after 15s the chat v2 session is
//      closed (and the chat session id is persisted first so --resume
//      has a target).
//   2. re-activating the project within 15s CANCELS the pending reap —
//      nothing is closed.
//   3. a dismissed-but-FOREGROUND project is never scheduled (rule 2).
//
// vitest env is `node` (no Tauri). We mock the daemon/Tauri boundaries
// the tabs module touches so importing it is inert, and spy on the two
// real teardown calls (v2 close via fetch, set-chat-session via
// workspace-agent) to prove the decision logic without a live daemon.

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

// The v2 close path resolves daemon creds then issues `fetch`. Give it a
// well-shaped creds object so `closeV2Session` builds a URL and calls
// fetch (which we spy on), rather than throwing during cred resolution.
const closedAgentNames: string[] = []
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))

// set-chat-session persistence — spy so we can assert it fires BEFORE
// the reap, and so the chat session id is stamped to the daemon DB.
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
} from './tabs'

// ── Test harness ─────────────────────────────────────────────────────────

// Foreground project id, swappable per-test. tabs.ts reads this via the
// registered getter (the projects→tabs cycle shim).
let foregroundProjectId: string | null = null
registerActiveProjectIdGetter(() => foregroundProjectId)

// Spy on global fetch so the v2 close call is observable. The body
// carries `{ agent_name }`; the reap closes the chat session under the
// bare projectId agent_name.
const fetchSpy = vi.fn(async (_url: string, init?: { body?: string }) => {
  if (init?.body) {
    try {
      const parsed = JSON.parse(init.body)
      if (parsed.agent_name) closedAgentNames.push(parsed.agent_name)
    } catch { /* ignore */ }
  }
  return { ok: true, status: 200, text: async () => 'ok' } as unknown as Response
})

// A pinned Chat agent item carrying a persisted Claude session id, in a
// background-workspace snapshot keyed by the dismissed project.
const CHAT_SESSION_ID = 'claude-session-uuid-657'
function seedDismissedChatItem(projectId: string, projectPath: string): void {
  useTabsStore.setState({
    backgroundWorkspaces: {
      [`${projectId}:ws1`]: {
        tabs: [
          {
            id: 'tab-chat',
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

describe('#657 dismiss-reap grace timer (renderer)', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    foregroundProjectId = null
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

  it('schedules a reap that fires after the 15s grace and closes the chat v2 session', async () => {
    const projectId = 'proj-A'
    const projectPath = '/work/proj-A'
    foregroundProjectId = 'proj-OTHER' // dismissed project is NOT foreground
    seedDismissedChatItem(projectId, projectPath)

    useTabsStore.getState().scheduleWorkspaceChatReap(projectId, projectPath)
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    // Nothing fires before the grace window elapses.
    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS - 1)
    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)

    // Cross the threshold. The timer callback persists the session id,
    // then (on the resolved promise) reaps. Flush microtasks so the
    // promise chain runs to completion.
    vi.advanceTimersByTime(1)
    await vi.runAllTimersAsync()

    // The chat session id was persisted to the daemon DB BEFORE reaping.
    expect(setChatSessionCalls).toEqual([
      { projectPath, sessionId: CHAT_SESSION_ID },
    ])
    // The pinned chat v2 session was closed under the bare projectId
    // agent_name (AgentChatPane's `attachAgentName={projectId}`).
    expect(closedAgentNames).toContain(projectId)
    // Timer entry is cleared after firing.
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)
  })

  it('re-activating the project within 15s CANCELS the pending reap', async () => {
    const projectId = 'proj-B'
    const projectPath = '/work/proj-B'
    foregroundProjectId = 'proj-OTHER'
    seedDismissedChatItem(projectId, projectPath)

    useTabsStore.getState().scheduleWorkspaceChatReap(projectId, projectPath)
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    // User returns within the window.
    vi.advanceTimersByTime(5_000)
    useTabsStore.getState().cancelWorkspaceChatReap(projectId)
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    // Let any (incorrectly) surviving timer fire — it must not.
    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()

    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)
  })

  it('never schedules a reap for a dismissed-but-FOREGROUND project (rule 2)', async () => {
    const projectId = 'proj-C'
    const projectPath = '/work/proj-C'
    foregroundProjectId = projectId // the dismissed project IS foreground
    seedDismissedChatItem(projectId, projectPath)

    useTabsStore.getState().scheduleWorkspaceChatReap(projectId, projectPath)

    // No timer was ever created.
    expect(__hasPendingChatReapForTests(projectId)).toBe(false)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS * 2)
    await vi.runAllTimersAsync()

    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)
  })

  it('re-checks foreground at FIRE time and skips reaping if the user returned (defense in depth)', async () => {
    const projectId = 'proj-D'
    const projectPath = '/work/proj-D'
    foregroundProjectId = 'proj-OTHER'
    seedDismissedChatItem(projectId, projectPath)

    useTabsStore.getState().scheduleWorkspaceChatReap(projectId, projectPath)
    expect(__hasPendingChatReapForTests(projectId)).toBe(true)

    // User navigates back to the project but (hypothetically) the cancel
    // wiring didn't run — the fire-time foreground re-check must still
    // protect the warm PTY.
    foregroundProjectId = projectId
    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()

    expect(setChatSessionCalls.length).toBe(0)
    expect(closedAgentNames.length).toBe(0)
  })
})

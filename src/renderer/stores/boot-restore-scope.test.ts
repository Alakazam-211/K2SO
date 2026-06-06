// P2 / OPEN-1 — boot-restore scope + boot-survivor reach.
//
// LIVE-CORRECTED. The prior version of this suite asserted a single
// invariant and attached a comment claiming it PROVED "app boot does not
// keep any non-active workspace's chat session alive." A LIVE cold-boot
// smoke test falsified that conclusion: 11 pinned-Chat PTYs were alive
// (only ~4 active, 6 aged out) and survived multiple reaper ticks.
//
// The renderer half of the original claim is still TRUE and is locked in
// below: `loadWorkspaceSessionsFromDb` caches saved layouts but restores
// NONE into live tabs / `backgroundWorkspaces`, and `fetchProjects`
// mounts exactly ONE workspace. So the renderer never SPAWNS non-active
// PTYs at boot.
//
// The falsified part was the unstated leap "…therefore no aged-out hidden
// sessions exist to reap." They DO: the daemon keeps `agent-chat:<pid>`
// PTYs alive ACROSS app restarts (daemon-authoritative), so a workspace
// the user never re-opens this session still has a live daemon chat PTY.
// Those are invisible to any renderer-snapshot-fed sweep. The boot-time
// daemon sweep (`sweepAgedOutWorkspaceChatsFromDaemon`, run from the
// ActiveBar effect as soon as `projects` hydrate) is what reaps them.
//
// This suite locks BOTH halves: (1) the renderer restores only the
// active workspace at boot, and (2) the daemon sweep reaps the aged-out
// boot survivors the renderer never mounted.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

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

// Hoisted so the vi.mock factory (hoisted above top-level consts) can
// close over the saved-layout list + the mutable running-PTY list without
// a TDZ on the import-time `loadWorkspaceSessionsFromDb()` call.
const h = vi.hoisted(() => {
  // The boot loader fetches saved layouts from the daemon. Return two
  // workspaces' worth so we can prove the loader caches BOTH without
  // restoring EITHER into live tabs / background snapshots.
  const SAVED = [
    { projectId: 'projA', workspaceId: 'wsA', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
    { projectId: 'projB', workspaceId: 'wsB', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
  ]
  // The daemon's live PTY list — controllable per-test so we can model
  // boot survivors (aged-out chat PTYs alive from a prior app session).
  const state: { runningPtys: Array<{ terminalId: string; cwd: string; command: string | null }> } = {
    runningPtys: [],
  }
  return { SAVED, state }
})
function setRunningPtys(list: Array<{ terminalId: string; cwd: string; command: string | null }>): void {
  h.state.runningPtys = list
}
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    if (route === 'workspace-layouts/load-all') return h.SAVED
    if (route === 'terminal/list-running') return h.state.runningPtys
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
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
  setChatSession: vi.fn(async () => undefined),
}))

import {
  useTabsStore,
  registerActiveProjectIdGetter,
  DISMISS_REAP_GRACE_MS,
  __clearPendingChatReapsForTests,
  __hasPendingChatReapForTests,
  type AgeOutProjectMeta,
} from './tabs'
import { agentChatId } from '@/lib/terminal-id'

describe('OPEN-1 — boot-restore does not spawn non-active sessions', () => {
  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      backgroundWorkspaces: {},
      workspaceLayouts: {},
      extraGroups: [],
    })
  })

  it('loadWorkspaceSessionsFromDb caches every saved layout but restores none', async () => {
    await useTabsStore.getState().loadWorkspaceSessionsFromDb()
    const s = useTabsStore.getState()

    // Both saved workspaces are cached in the in-memory layout map…
    expect(Object.keys(s.workspaceLayouts).sort()).toEqual(['projA:wsA', 'projB:wsB'])

    // …but NONE are restored: no live tabs, no background snapshots. The
    // renderer never SPAWNS a non-active workspace's PTY at boot — it only
    // mounts the ONE workspace `fetchProjects` restores. (A regression that
    // iterated backgroundWorkspaces / built tabs at boot would fail here.)
    expect(s.tabs).toEqual([])
    expect(s.backgroundWorkspaces).toEqual({})
    expect(s.activeTabId).toBeNull()
  })
})

describe('LIVE-fix — boot-time daemon sweep reaps aged-out boot survivors', () => {
  const fetchSpy = vi.fn(async (_url: string, init?: { body?: string }) => {
    if (init?.body) {
      try {
        const parsed = JSON.parse(init.body)
        if (parsed.agent_name) closedAgentNames.push(parsed.agent_name)
      } catch { /* ignore */ }
    }
    return { ok: true, status: 200, text: async () => 'ok' } as unknown as Response
  })

  beforeEach(() => {
    vi.useFakeTimers()
    closedAgentNames.length = 0
    fetchSpy.mockClear()
    vi.stubGlobal('fetch', fetchSpy)
    __clearPendingChatReapsForTests()
    setRunningPtys([])
    // The active workspace at boot is `projA` — never reaped.
    registerActiveProjectIdGetter(() => 'projA')
    // Boot survivors are daemon-side ONLY: no renderer tabs/snapshots.
    useTabsStore.setState({ tabs: [], backgroundWorkspaces: {} })
  })

  afterEach(() => {
    __clearPendingChatReapsForTests()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('reaps the aged-out survivors but spares the active one — exactly the LIVE scenario', async () => {
    // Model the LIVE boot: the daemon holds chat PTYs from a prior app
    // session. `projA` is the workspace the user landed on (active, fresh);
    // `projAged1`/`projAged2` aged out while the user was away and were
    // never opened this session.
    setRunningPtys([
      { terminalId: agentChatId('projA', ''), cwd: '/work/projA', command: 'claude' },
      { terminalId: agentChatId('projAged1', ''), cwd: '/work/projAged1', command: 'claude' },
      { terminalId: agentChatId('projAged2', ''), cwd: '/work/projAged2', command: 'claude' },
    ])

    const metaByProjectId: Record<string, AgeOutProjectMeta> = {
      // Active + fresh → spared (foreground gate + isAged=false both hold).
      projA: { projectPath: '/work/projA', isAged: false, manuallyActive: false, heartbeatEnabled: false },
      projAged1: { projectPath: '/work/projAged1', isAged: true, manuallyActive: false, heartbeatEnabled: false },
      projAged2: { projectPath: '/work/projAged2', isAged: true, manuallyActive: false, heartbeatEnabled: false },
    }

    await useTabsStore.getState().sweepAgedOutWorkspaceChatsFromDaemon(metaByProjectId)

    expect(__hasPendingChatReapForTests('projA')).toBe(false)
    expect(__hasPendingChatReapForTests('projAged1')).toBe(true)
    expect(__hasPendingChatReapForTests('projAged2')).toBe(true)

    vi.advanceTimersByTime(DISMISS_REAP_GRACE_MS)
    await vi.runAllTimersAsync()

    expect(closedAgentNames.sort()).toEqual(['projAged1', 'projAged2'])
    expect(closedAgentNames).not.toContain('projA')
  })
})

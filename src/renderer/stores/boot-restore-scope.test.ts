// P2 / OPEN-1 — boot-restore scope invariant.
//
// Investigation finding: app boot does NOT restore or spawn sessions
// for non-active workspaces. The boot import (`loadWorkspaceSessionsFromDb`)
// only hydrates the in-memory `workspaceLayouts` CACHE — it does not
// build tabs, does not populate `backgroundWorkspaces`, and does not
// mount any pane (PTYs spawn lazily on AgentChatPane/TerminalPane
// mount, which happens only for the ACTIVE workspace's tabs). The
// projects store's boot path (`fetchProjects`) restores exactly ONE
// workspace — the last-active one — via `loadLayoutForWorkspace`;
// `backgroundWorkspaces` starts empty and is only filled by a
// switch-away `stashWorkspace`.
//
// This suite locks that invariant in: a regression that started
// iterating `backgroundWorkspaces` (or building tabs) at boot — the
// hypothetical OPEN-1 source of lingering sessions — would fail here.

import { describe, it, expect, beforeEach, vi } from 'vitest'

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

// The boot loader fetches saved layouts from the daemon. Return two
// workspaces' worth of saved layouts so we can prove the loader caches
// BOTH without restoring EITHER into live tabs / background snapshots.
const SAVED = [
  { projectId: 'projA', workspaceId: 'wsA', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
  { projectId: 'projB', workspaceId: 'wsB', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
]
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    if (route === 'workspace-layouts/load-all') return SAVED
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
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
  setChatSession: vi.fn(async () => undefined),
}))

import { useTabsStore } from './tabs'

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

    // …but NONE are restored: no live tabs, no background snapshots.
    // (A regression that iterated backgroundWorkspaces / built tabs at
    // boot — the OPEN-1 lingering-session hazard — would fail here.)
    expect(s.tabs).toEqual([])
    expect(s.backgroundWorkspaces).toEqual({})
    expect(s.activeTabId).toBeNull()
  })
})

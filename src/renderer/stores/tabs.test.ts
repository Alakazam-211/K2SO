import { describe, it, expect, beforeEach } from 'vitest'
import { useTabsStore, type AgentItemData } from './tabs'

/**
 * Tests for the post-0.36.0 pinned-agent-tab split behaviour.
 *
 * Pre-split: a single isSystemAgent tab held the agent's UI as 4
 * sub-tabs (Work / Chat / CLAUDE.md / Profile).
 *
 * Post-split: up to TWO isSystemAgent tabs per workspace —
 *   - section 'inbox' (always)
 *   - section 'chat'  (skipped for the workspace board)
 */

function reset(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
  })
}

function getAgentItem(tabIndex: number): AgentItemData | null {
  const tab = useTabsStore.getState().tabs[tabIndex]
  if (!tab) return null
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (item?.type !== 'agent') return null
  return item.data as AgentItemData
}

describe('ensureSystemAgentTabs', () => {
  beforeEach(reset)

  it('creates two pinned tabs (Inbox + Chat) for a regular agent', () => {
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    const tabs = useTabsStore.getState().tabs
    const systemTabs = tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)

    // Canonical order: Chat first, Inbox second.
    expect(systemTabs[0].title).toBe('Chat')
    expect(systemTabs[1].title).toBe('Inbox')

    const chatItem = getAgentItem(0)
    const inboxItem = getAgentItem(1)
    expect(chatItem?.section).toBe('chat')
    expect(inboxItem?.section).toBe('inbox')
    expect(chatItem?.agentName).toBe('manager')
    expect(inboxItem?.agentName).toBe('manager')
  })

  it('forces canonical order even when only one section pre-existed', () => {
    // Simulate a half-migrated layout: only the Inbox tab is in place.
    useTabsStore.setState({
      tabs: [{
        id: 'existing-inbox',
        title: 'Inbox',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: { agentName: 'manager', projectPath: '/tmp/proj', section: 'inbox' },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'existing-inbox',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    // After the call, Chat must come first even though Inbox was the
    // only pre-existing tab. Pre-existing tab keeps its id (state
    // preservation); only ordering changes.
    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
    expect(systemTabs[0].title).toBe('Chat')
    expect(systemTabs[1].title).toBe('Inbox')
    expect(systemTabs[1].id).toBe('existing-inbox')
  })

  it('creates only the Inbox tab for the workspace board (no chat surface)', () => {
    useTabsStore.getState().ensureSystemAgentTabs('__workspace__', '/tmp/proj', 'Work Board')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(1)
    expect(systemTabs[0].title).toBe('Work Board')

    const item = getAgentItem(0)
    expect(item?.section).toBe('inbox')
    expect(item?.agentName).toBe('__workspace__')
  })

  it('is idempotent — calling twice does not create duplicates', () => {
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
  })

  it('back-fills a missing section if only one pinned tab exists', () => {
    // Simulate a half-migrated state: only the inbox tab is in place.
    useTabsStore.setState({
      tabs: [{
        id: 'existing-inbox',
        title: 'Inbox',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: { agentName: 'manager', projectPath: '/tmp/proj', section: 'inbox' },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'existing-inbox',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
    const sections = systemTabs
      .map((t) => {
        const it = Array.from(t.paneGroups.values())[0]?.items[0]
        return it?.type === 'agent' ? (it.data as AgentItemData).section : null
      })
      .filter((s) => s !== null)
    expect(sections).toContain('inbox')
    expect(sections).toContain('chat')
  })

  it('heals a restored pinned tab carrying a stale workspace path', () => {
    // Repro: HK47's saved layout has a pinned Chat tab whose agent item
    // still points at the K2SO workspace (projectPath baked in under the
    // wrong workspace and replayed verbatim by restoreLayout). Switching
    // into HK47 calls ensureSystemAgentTabs with HK47's authoritative
    // path — the existing tab must be reconciled, not reused as-is.
    useTabsStore.setState({
      tabs: [{
        id: 'stale-chat',
        title: 'Chat',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: {
              agentName: 'k2so-manager',
              projectPath: '/workspaces/K2SO',
              section: 'chat',
              sessionId: 'old-k2so-session',
            },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'stale-chat',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('hk47-manager', '/workspaces/HK47', 'Manager')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    const chatTab = systemTabs.find((t) => t.title === 'Chat')!
    const chatData = (() => {
      const it = Array.from(chatTab.paneGroups.values())[0].items[0]
      return it.type === 'agent' ? (it.data as AgentItemData) : null
    })()

    // Reconciled to HK47, not left pointing at K2SO.
    expect(chatData?.projectPath).toBe('/workspaces/HK47')
    expect(chatData?.agentName).toBe('hk47-manager')
    // The Claude session belonged to the old workspace — dropped.
    expect(chatData?.sessionId).toBeUndefined()
    // Tab identity is preserved (state continuity), only data changed.
    expect(chatTab.id).toBe('stale-chat')
  })

  it('does not mutate a pinned tab that already matches the workspace', () => {
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    const before = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)

    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    const after = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)

    // Same tab object references — no needless re-render when nothing changed.
    expect(after[0]).toBe(before[0])
    expect(after[1]).toBe(before[1])
  })

  it('inserts pinned tabs at the front of the strip', () => {
    // Seed a non-system tab first.
    useTabsStore.setState((s) => ({
      tabs: [
        ...s.tabs,
        {
          id: 'user-tab',
          title: 'README.md',
          mosaicTree: 'pg-x',
          paneGroups: new Map([['pg-x', {
            id: 'pg-x',
            items: [],
            activeItemIndex: 0,
          }]]),
        },
      ],
    }))

    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')

    const tabs = useTabsStore.getState().tabs
    expect(tabs[0].isSystemAgent).toBe(true)
    expect(tabs[1].isSystemAgent).toBe(true)
    expect(tabs[2].id).toBe('user-tab')
  })
})

describe('removeSystemAgentTab', () => {
  beforeEach(reset)

  it('removes BOTH pinned tabs (inbox + chat) when called after the split', () => {
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)).toHaveLength(2)

    useTabsStore.getState().removeSystemAgentTab()
    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)).toHaveLength(0)
  })
})

describe('moveTabToGroup (cross-column drag)', () => {
  function makeTab(id: string, title: string): import('./tabs').Tab {
    return {
      id,
      title,
      mosaicTree: `pg-${id}`,
      paneGroups: new Map([
        [`pg-${id}`, { id: `pg-${id}`, items: [], activeItemIndex: 0 }],
      ]),
    }
  }

  beforeEach(() => {
    useTabsStore.setState({
      tabs: [makeTab('a', 'A'), makeTab('b', 'B')],
      activeTabId: 'a',
      splitCount: 2,
      extraGroups: [{ tabs: [makeTab('c', 'C')], activeTabId: 'c' }],
      activeGroupIndex: 0,
    })
  })

  it('moves a tab from group 0 to group 1 and activates it there', () => {
    useTabsStore.getState().moveTabToGroup(0, 1, 'a')

    const s = useTabsStore.getState()
    expect(s.tabs.map((t) => t.id)).toEqual(['b'])
    expect(s.extraGroups[0].tabs.map((t) => t.id)).toEqual(['c', 'a'])
    expect(s.extraGroups[0].activeTabId).toBe('a')
  })

  it('moves a tab from group 1 back to group 0', () => {
    useTabsStore.getState().moveTabToGroup(1, 0, 'c')

    const s = useTabsStore.getState()
    expect(s.tabs.map((t) => t.id)).toEqual(['a', 'b', 'c'])
    expect(s.extraGroups[0].tabs).toHaveLength(0)
    expect(s.activeTabId).toBe('c')
  })

  it('updates source activeTabId when moving the active tab away', () => {
    useTabsStore.getState().moveTabToGroup(0, 1, 'a')
    expect(useTabsStore.getState().activeTabId).toBe('b')
  })

  it('is a no-op when source and target groups match', () => {
    const before = useTabsStore.getState().tabs.map((t) => t.id)
    useTabsStore.getState().moveTabToGroup(0, 0, 'a')
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(before)
  })

  it('is a no-op when the tab does not exist in the source group', () => {
    const before = JSON.parse(JSON.stringify({
      tabs: useTabsStore.getState().tabs.map((t) => t.id),
      extra: useTabsStore.getState().extraGroups[0].tabs.map((t) => t.id),
    }))
    useTabsStore.getState().moveTabToGroup(0, 1, 'does-not-exist')
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(before.tabs)
    expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.id)).toEqual(before.extra)
  })
})

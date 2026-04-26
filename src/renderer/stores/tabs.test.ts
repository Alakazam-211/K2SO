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

    // Order: Inbox first, Chat second — matters for left-to-right pinned strip
    expect(systemTabs[0].title).toBe('Inbox')
    expect(systemTabs[1].title).toBe('Chat')

    const inboxItem = getAgentItem(0)
    const chatItem = getAgentItem(1)
    expect(inboxItem?.section).toBe('inbox')
    expect(chatItem?.section).toBe('chat')
    expect(inboxItem?.agentName).toBe('manager')
    expect(chatItem?.agentName).toBe('manager')
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

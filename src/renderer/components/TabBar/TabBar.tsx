import { useCallback, useState, useRef, useEffect } from 'react'
import { useTabsStore, type TerminalItemData } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { invoke } from '@tauri-apps/api/core'
import { startTabDrag } from '@/components/Terminal/TerminalArea'
import { showContextMenu } from '@/lib/context-menu'
import AgentCloseDialog from '@/components/AgentCloseDialog/AgentCloseDialog'

interface TabBarProps {
  cwd: string
  groupIndex?: number
}

export function TabBar({ cwd, groupIndex = 0 }: TabBarProps): React.JSX.Element {
  const tabs = useTabsStore((s) => groupIndex === 0 ? s.tabs : s.extraGroups[groupIndex - 1]?.tabs ?? [])
  const activeTabId = useTabsStore((s) => groupIndex === 0 ? s.activeTabId : s.extraGroups[groupIndex - 1]?.activeTabId ?? null)
  const splitCount = useTabsStore((s) => s.splitCount)
  const addTabToGroup = useTabsStore((s) => s.addTabToGroup)
  const removeTabFromGroup = useTabsStore((s) => s.removeTabFromGroup)
  const setActiveTabInGroup = useTabsStore((s) => s.setActiveTabInGroup)
  const splitTerminalArea = useTabsStore((s) => s.splitTerminalArea)
  const unsplitTerminalArea = useTabsStore((s) => s.unsplitTerminalArea)
  const reorderTabs = useTabsStore((s) => s.reorderTabs)
  const agentMap = useActiveAgentsStore((s) => s.agents)

  const handleAddTab = useCallback(() => {
    addTabToGroup(groupIndex, cwd)
  }, [addTabToGroup, groupIndex, cwd])

  const [pendingClose, setPendingClose] = useState<{ tabId: string; agents: ReturnType<typeof useActiveAgentsStore.getState>['getAgentsInTab'] } | null>(null)

  // ── Tab reorder state ──
  const [reorderDragIndex, setReorderDragIndex] = useState<number | null>(null)
  const [reorderDropIndex, setReorderDropIndex] = useState<number | null>(null)
  const reorderDropRef = useRef<number | null>(null)
  const reorderFromRef = useRef<number | null>(null)
  const tabBarRef = useRef<HTMLDivElement>(null)

  const handleTabMouseDown = useCallback((e: React.MouseEvent, index: number, tabId: string, tabTitle: string) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement).closest('button')) return

    e.preventDefault() // Prevent text selection from starting

    const startX = e.clientX
    const startY = e.clientY
    let started = false
    let mode: 'reorder' | 'cross-group' | null = null

    // Block all selection during drag
    const blockSelect = (ev: Event): void => ev.preventDefault()
    document.addEventListener('selectstart', blockSelect)

    // Get the column bounds so we can detect when cursor leaves
    const columnEl = (e.currentTarget as HTMLElement).closest('[data-tab-group-index]') as HTMLElement | null
    const columnRect = columnEl?.getBoundingClientRect()

    const handleMouseMove = (ev: MouseEvent): void => {
      const dx = ev.clientX - startX
      const dy = ev.clientY - startY

      if (!started && (Math.abs(dx) > 3 || Math.abs(dy) > 5)) {
        started = true

        // If multiple columns and dragging vertically, go straight to cross-group
        if (splitCount > 1 && Math.abs(dy) > Math.abs(dx) * 1.5) {
          mode = 'cross-group'
          document.removeEventListener('selectstart', blockSelect)
          startTabDrag({ groupIndex, tabId, tabTitle, mouseX: ev.clientX, mouseY: ev.clientY })
          document.removeEventListener('mousemove', handleMouseMove)
          document.removeEventListener('mouseup', handleMouseUp)
          return
        }

        mode = 'reorder'
        reorderFromRef.current = index
        setReorderDragIndex(index)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }

      if (!started || mode !== 'reorder') return

      // If cursor leaves the column bounds, switch to cross-group drag
      if (splitCount > 1 && columnRect) {
        if (ev.clientX < columnRect.left - 10 || ev.clientX > columnRect.right + 10 ||
            ev.clientY < columnRect.top - 30 || ev.clientY > columnRect.bottom + 30) {
          mode = 'cross-group'
          setReorderDragIndex(null)
          setReorderDropIndex(null)
          reorderDropRef.current = null
          reorderFromRef.current = null
          document.body.style.cursor = ''
          document.body.style.userSelect = ''
          document.removeEventListener('selectstart', blockSelect)
          startTabDrag({ groupIndex, tabId, tabTitle, mouseX: ev.clientX, mouseY: ev.clientY })
          document.removeEventListener('mousemove', handleMouseMove)
          document.removeEventListener('mouseup', handleMouseUp)
          return
        }
      }

      // Find which tab slot the cursor is over
      if (!tabBarRef.current) return
      const items = tabBarRef.current.querySelectorAll<HTMLElement>('[data-tab-reorder-index]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientX > rect.left + rect.width / 2) dropIdx = i + 1
      }
      reorderDropRef.current = dropIdx
      setReorderDropIndex(dropIdx)
    }

    const handleMouseUp = (): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.removeEventListener('selectstart', blockSelect)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started && mode === 'reorder') {
        const fromIdx = reorderFromRef.current
        const dropIdx = reorderDropRef.current
        if (fromIdx !== null && dropIdx !== null && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
          const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
          reorderTabs(fromIdx, insertAt, groupIndex)
        }
      }

      setReorderDragIndex(null)
      setReorderDropIndex(null)
      reorderDropRef.current = null
      reorderFromRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [groupIndex, splitCount, reorderTabs])

  const handleCloseTab = useCallback(
    (e: React.MouseEvent, tabId: string) => {
      e.stopPropagation()
      const agents = useActiveAgentsStore.getState().getAgentsInTab(tabId)
      if (agents.length > 0) {
        setPendingClose({ tabId, agents })
      } else {
        removeTabFromGroup(groupIndex, tabId)
      }
    },
    [removeTabFromGroup, groupIndex]
  )

  const handleTabContextMenu = useCallback(async (e: React.MouseEvent, tabId: string) => {
    e.preventDefault()
    e.stopPropagation()

    // Get the default terminal name from settings
    const settings = useSettingsStore.getState()
    const defaultTerminal = (settings.projectSettings['__global__'] as any)?.defaultTerminal ?? 'Terminal'

    const allTabs = groupIndex === 0 ? useTabsStore.getState().tabs : useTabsStore.getState().extraGroups[groupIndex - 1]?.tabs ?? []
    const hasOtherTabs = allTabs.length > 1

    const menuItems = [
      { id: 'open-terminal', label: `Open in ${defaultTerminal}` },
      { id: 'separator', label: '', type: 'separator' as const },
      { id: 'close', label: 'Close Tab' },
      ...(hasOtherTabs ? [
        { id: 'close-others', label: 'Close Other Tabs' },
        { id: 'close-all', label: 'Close All Tabs' },
      ] : []),
    ]

    const clickedId = await showContextMenu(menuItems)
    if (clickedId === 'close') {
      const agents = useActiveAgentsStore.getState().getAgentsInTab(tabId)
      if (agents.length > 0) {
        setPendingClose({ tabId, agents })
      } else {
        removeTabFromGroup(groupIndex, tabId)
      }
    } else if (clickedId === 'close-others') {
      // Close all tabs except the right-clicked one
      for (const tab of allTabs) {
        if (tab.id !== tabId) {
          removeTabFromGroup(groupIndex, tab.id)
        }
      }
    } else if (clickedId === 'close-all') {
      for (const tab of allTabs) {
        removeTabFromGroup(groupIndex, tab.id)
      }
    } else if (clickedId === 'open-terminal') {
      // Find the cwd from the tab's first terminal pane
      const tabsState = useTabsStore.getState()
      const allTabs = groupIndex === 0 ? tabsState.tabs : tabsState.extraGroups[groupIndex - 1]?.tabs ?? []
      const tab = allTabs.find((t) => t.id === tabId)
      if (tab) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const termCwd = (item.data as TerminalItemData).cwd
              // Open the cwd in the default terminal app
              if (defaultTerminal === 'Terminal') {
                import('@tauri-apps/plugin-opener').then(({ openPath }) => openPath(termCwd)).catch((e) => console.warn('[tab-bar]', e))
              } else {
                // Use the terminal app's CLI to open
                invoke('projects_open_in_terminal', { terminalApp: defaultTerminal, path: termCwd }).catch(() => {
                  // Fallback to generic open
                  import('@tauri-apps/plugin-opener').then(({ openPath }) => openPath(termCwd)).catch((e) => console.warn('[tab-bar]', e))
                })
              }
              return
            }
          }
        }
      }
    }
  }, [groupIndex, removeTabFromGroup])

  // Scroll active tab into view when it changes
  useEffect(() => {
    if (!activeTabId || !tabBarRef.current) return
    const container = tabBarRef.current
    const activeEl = container.querySelector(`[data-tab-id="${activeTabId}"]`) as HTMLElement | null
    if (activeEl) {
      activeEl.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'nearest' })
    }
  }, [activeTabId])

  return (
    <div
      className="flex h-9 items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] no-drag"
    >
      <div ref={tabBarRef} className="flex h-full flex-1 items-center overflow-x-auto tabbar-scroll">
        {tabs.map((tab, index) => {
          const isActive = tab.id === activeTabId
          const isDirty = tab.isDirty ?? false
          const agentsInTab = Array.from(agentMap.values()).filter(a => a.tabId === tab.id)
          const hasAgent = agentsInTab.length > 0
          const hasActiveAgent = agentsInTab.some(a => a.status === 'active')
          const isDragged = reorderDragIndex === index
          const showDropBefore = reorderDropIndex === index
          const showDropAfter = reorderDropIndex === tabs.length && index === tabs.length - 1

          return (
            <div
              key={tab.id}
              data-tab-id={tab.id}
              data-tab-reorder-index={index}
              className={`group relative flex h-full min-w-[100px] max-w-[200px] flex-shrink-0 items-center border-r border-[var(--color-border)] px-3 text-xs transition-colors select-none ${
                isActive
                  ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
              } ${isDragged ? 'opacity-30' : ''}`}
              onClick={() => setActiveTabInGroup(groupIndex, tab.id)}
              onContextMenu={(e) => handleTabContextMenu(e, tab.id)}
              onMouseDown={(e) => handleTabMouseDown(e, index, tab.id, tab.title)}
            >
              {showDropBefore && <div className="absolute left-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10" />}
              {showDropAfter && <div className="absolute right-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10" />}
              {hasAgent && (
                <span
                  className={`flex-shrink-0 mr-1.5 rounded-full ${hasActiveAgent ? 'agent-active-dot' : ''}`}
                  style={{ width: 6, height: 6, backgroundColor: hasActiveAgent ? '#f97316' : '#22c55e' }}
                />
              )}
              {isDirty && !hasAgent && (
                <span className="w-1.5 h-1.5 bg-[var(--color-accent)] flex-shrink-0 mr-1.5" />
              )}
              <span className={`truncate flex-1 ${isDirty ? 'italic' : ''}`}>
                {tab.title}
              </span>
              <button
                className="ml-2 flex h-4 w-4 flex-shrink-0 items-center justify-center hover:bg-white/10"
                onClick={(e) => handleCloseTab(e, tab.id)}
                title="Close tab"
              >
                <svg
                  width="8"
                  height="8"
                  viewBox="0 0 8 8"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                >
                  <line x1="1" y1="1" x2="7" y2="7" />
                  <line x1="7" y1="1" x2="1" y2="7" />
                </svg>
              </button>
            </div>
          )
        })}
      </div>

      {/* Only show split/unsplit on the rightmost group's tab bar */}
      {groupIndex === splitCount - 1 && (
        <>
          {/* Split button */}
          {splitCount < 3 && (
            <button
              className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
              onClick={() => splitTerminalArea(cwd)}
              title="Split into columns"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.3"
              >
                <rect x="0.5" y="0.5" width="11" height="11" />
                <line x1="6" y1="0.5" x2="6" y2="11.5" />
              </svg>
            </button>
          )}

          {/* Unsplit button — only when split */}
          {splitCount > 1 && (
            <button
              className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
              onClick={unsplitTerminalArea}
              title="Remove column"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.3"
              >
                <rect x="0.5" y="0.5" width="11" height="11" />
              </svg>
            </button>
          )}
        </>
      )}

      {/* Add tab button */}
      <button
        className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
        onClick={handleAddTab}
        title="New tab (Cmd+T)"
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
        >
          <line x1="6" y1="1" x2="6" y2="11" />
          <line x1="1" y1="6" x2="11" y2="6" />
        </svg>
      </button>
      {/* Agent close confirmation dialog */}
      {pendingClose && (
        <AgentCloseDialog
          agents={pendingClose.agents}
          mode="tab"
          onConfirm={() => {
            removeTabFromGroup(groupIndex, pendingClose.tabId)
            setPendingClose(null)
          }}
          onCancel={() => setPendingClose(null)}
        />
      )}
    </div>
  )
}

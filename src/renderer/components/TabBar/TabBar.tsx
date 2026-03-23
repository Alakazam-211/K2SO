import { useCallback, useState } from 'react'
import { useTabsStore } from '@/stores/tabs'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { startTabDrag } from '@/components/Terminal/TerminalArea'
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

  const handleAddTab = useCallback(() => {
    addTabToGroup(groupIndex, cwd)
  }, [addTabToGroup, groupIndex, cwd])

  const [pendingClose, setPendingClose] = useState<{ tabId: string; agents: ReturnType<typeof useActiveAgentsStore.getState>['getAgentsInTab'] } | null>(null)

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

  const handleTabMouseDown = useCallback((e: React.MouseEvent, tabId: string, tabTitle: string) => {
    // Only start drag on left click, not on close button
    if (e.button !== 0) return
    if ((e.target as HTMLElement).closest('button')) return

    // Only enable cross-group drag when there are multiple groups
    const store = useTabsStore.getState()
    if (store.splitCount <= 1) return

    const startX = e.clientX
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      // Require a small drag distance before starting
      if (!started && (Math.abs(ev.clientX - startX) > 5 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        startTabDrag({ groupIndex, tabId, tabTitle, mouseX: ev.clientX, mouseY: ev.clientY })
      }
    }

    const handleMouseUp = (): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [groupIndex])

  return (
    <div
      className="flex h-9 items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] no-drag"
    >
      <div className="flex h-full flex-1 items-center overflow-x-auto">
        {tabs.map((tab) => {
          const isActive = tab.id === activeTabId
          const isDirty = tab.isDirty ?? false
          const hasAgent = useActiveAgentsStore.getState().getAgentsInTab(tab.id).length > 0
          return (
            <div
              key={tab.id}
              className={`group flex h-full min-w-0 max-w-[200px] cursor-pointer items-center border-r border-[var(--color-border)] px-3 text-xs transition-colors select-none ${
                isActive
                  ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
              }`}
              onClick={() => setActiveTabInGroup(groupIndex, tab.id)}
              onMouseDown={(e) => handleTabMouseDown(e, tab.id, tab.title)}
            >
              {hasAgent && (
                <span className="agent-active-dot flex-shrink-0 mr-1.5" style={{ width: 6, height: 6, backgroundColor: '#22c55e' }} />
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

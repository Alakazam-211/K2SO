import { useRef, useCallback } from 'react'
import { useTabsStore } from '@/stores/tabs'

interface TabBarProps {
  cwd: string
}

export function TabBar({ cwd }: TabBarProps): React.JSX.Element {
  const { tabs, activeTabId, addTab, removeTab, setActiveTab, reorderTabs } =
    useTabsStore()

  const dragIndexRef = useRef<number | null>(null)

  const handleAddTab = useCallback(() => {
    addTab(cwd)
  }, [addTab, cwd])

  const handleCloseTab = useCallback(
    (e: React.MouseEvent, tabId: string) => {
      e.stopPropagation()
      removeTab(tabId)
    },
    [removeTab]
  )

  const handleDragStart = useCallback((e: React.DragEvent, index: number) => {
    dragIndexRef.current = index
    e.dataTransfer.effectAllowed = 'move'
    // Make the drag image semi-transparent
    if (e.currentTarget instanceof HTMLElement) {
      e.dataTransfer.setDragImage(e.currentTarget, 0, 0)
    }
  }, [])

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.dataTransfer.dropEffect = 'move'
  }, [])

  const handleDrop = useCallback(
    (e: React.DragEvent, toIndex: number) => {
      e.preventDefault()
      const fromIndex = dragIndexRef.current
      if (fromIndex !== null && fromIndex !== toIndex) {
        reorderTabs(fromIndex, toIndex)
      }
      dragIndexRef.current = null
    },
    [reorderTabs]
  )

  return (
    <div className="flex h-9 items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] no-drag">
      <div className="flex h-full flex-1 items-center overflow-x-auto">
        {tabs.map((tab, index) => {
          const isActive = tab.id === activeTabId
          return (
            <div
              key={tab.id}
              className={`group flex h-full min-w-0 max-w-[180px] cursor-pointer items-center gap-1 border-r border-[var(--color-border)] px-3 text-xs transition-colors ${
                isActive
                  ? 'bg-[var(--color-bg)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)]'
              }`}
              onClick={() => setActiveTab(tab.id)}
              draggable
              onDragStart={(e) => handleDragStart(e, index)}
              onDragOver={handleDragOver}
              onDrop={(e) => handleDrop(e, index)}
            >
              <span className="truncate">{tab.title}</span>
              <button
                className="ml-auto flex h-4 w-4 flex-shrink-0 items-center justify-centeropacity-0 transition-opacity hover:bg-white/10 group-hover:opacity-100"
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
    </div>
  )
}

import { useEffect, useRef } from 'react'
import { TabBar } from '@/components/TabBar/TabBar'
import { PaneLayout } from '@/components/PaneLayout/PaneLayout'
import { PresetsBar } from '@/components/PresetsBar/PresetsBar'
import { useTabsStore } from '@/stores/tabs'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'

interface TerminalAreaProps {
  cwd: string
}

export function TerminalArea({ cwd }: TerminalAreaProps): React.JSX.Element {
  const { tabs, activeTabId, addTab } = useTabsStore()
  const prevActiveTabRef = useRef<string | null>(null)

  // Register keyboard shortcuts
  useTerminalShortcuts(cwd)

  // Create initial tab if none exist
  useEffect(() => {
    if (tabs.length === 0) {
      addTab(cwd)
    }
  }, []) // intentionally run only once

  // Focus the terminal when the active tab changes
  useEffect(() => {
    if (activeTabId && activeTabId !== prevActiveTabRef.current) {
      prevActiveTabRef.current = activeTabId
      // Small delay to let the tab render, then focus the first xterm in the active tab
      requestAnimationFrame(() => {
        const activeTabEl = document.querySelector(`[data-tab-id="${activeTabId}"]`)
        if (activeTabEl) {
          const textarea = activeTabEl.querySelector('.xterm-helper-textarea') as HTMLTextAreaElement
          if (textarea) textarea.focus()
        }
      })
    }
  }, [activeTabId])

  return (
    <div className="flex h-full w-full flex-col overflow-hidden">
      <TabBar cwd={cwd} />
      <PresetsBar cwd={cwd} />
      <div className="relative flex-1 overflow-hidden">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            data-tab-id={tab.id}
            className="absolute inset-0"
            style={{ display: tab.id === activeTabId ? 'block' : 'none' }}
          >
            <PaneLayout tabId={tab.id} />
          </div>
        ))}
        {tabs.length === 0 && (
          <div className="flex h-full items-center justify-center text-sm text-[var(--color-text-muted)]">
            Press Cmd+T to open a new terminal tab
          </div>
        )}
      </div>
    </div>
  )
}

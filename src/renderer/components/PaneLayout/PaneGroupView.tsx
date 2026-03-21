import { useCallback } from 'react'
import { TerminalView } from '@/components/Terminal/TerminalView'
import { FileViewerPane } from '@/components/FileViewerPane/FileViewerPane'
import { useTabsStore } from '@/stores/tabs'
import type { TerminalItemData, FileViewerItemData } from '@/stores/tabs'
import { PaneTabBar } from './PaneTabBar'

// ── Props ────────────────────────────────────────────────────────────────

interface PaneGroupViewProps {
  tabId: string
  paneGroupId: string
}

// ── Component ────────────────────────────────────────────────────────────

export function PaneGroupView({ tabId, paneGroupId }: PaneGroupViewProps): React.JSX.Element {
  const paneGroup = useTabsStore((s) => {
    let tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) {
      for (const g of s.extraGroups) {
        tab = g.tabs.find((t) => t.id === tabId)
        if (tab) break
      }
    }
    if (!tab || !tab.paneGroups) return undefined
    return tab.paneGroups.get(paneGroupId)
  })

  const activateItem = useTabsStore((s) => s.activateItemInPaneGroup)
  const closeItem = useTabsStore((s) => s.closeItemInPaneGroup)
  const removePaneFromTab = useTabsStore((s) => s.removePaneFromTab)

  // Check if this is a split (more than one pane in the mosaic tree)
  const hasSplits = useTabsStore((s) => {
    let tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) {
      for (const g of s.extraGroups) {
        tab = g.tabs.find((t) => t.id === tabId)
        if (tab) break
      }
    }
    if (!tab || !tab.paneGroups) return false
    return tab.paneGroups.size > 1
  })

  const handleActivate = useCallback(
    (index: number) => {
      activateItem?.(tabId, paneGroupId, index)
    },
    [tabId, paneGroupId, activateItem]
  )

  const handleClose = useCallback(
    (itemId: string) => {
      closeItem?.(tabId, paneGroupId, itemId)
    },
    [tabId, paneGroupId, closeItem]
  )

  const handleClosePane = useCallback(() => {
    removePaneFromTab(tabId, paneGroupId)
  }, [tabId, paneGroupId, removePaneFromTab])

  // ── Empty state ────────────────────────────────────────────────────────
  if (!paneGroup || !paneGroup.items || paneGroup.items.length === 0) {
    return (
      <div className="flex h-full w-full flex-col">
        <div
          className="flex items-center border-b border-[var(--color-border)]"
          style={{
            height: '24px',
            minHeight: '24px',
            background: '#111',
            fontSize: '11px',
            fontFamily: "'MesloLGM Nerd Font', Menlo, Monaco, monospace"
          }}
        />
        <div className="flex flex-1 items-center justify-center text-[var(--color-text-muted)]" style={{ fontSize: '11px' }}>
          Empty pane
        </div>
      </div>
    )
  }

  // ── Active item ────────────────────────────────────────────────────────
  const activeIndex = Math.min(paneGroup.activeItemIndex, paneGroup.items.length - 1)
  const activeItem = paneGroup.items[activeIndex]

  if (!activeItem) {
    return (
      <div className="flex h-full w-full flex-col">
        <PaneTabBar
          items={paneGroup.items}
          activeItemIndex={activeIndex}
          onActivate={handleActivate}
          onClose={handleClose}
          onClosePane={hasSplits ? handleClosePane : undefined}
        />
        <div className="flex flex-1 items-center justify-center text-[var(--color-text-muted)]" style={{ fontSize: '11px' }}>
          Empty pane
        </div>
      </div>
    )
  }

  const terminalData = activeItem.type === 'terminal' && activeItem.data as TerminalItemData
  const fileData = activeItem.type === 'file-viewer' && activeItem.data as FileViewerItemData

  // Only show per-pane tab bar when there are splits
  // (so the close-pane button is accessible).
  // Single-pane items are managed by the workspace TabBar at the top.
  const showPaneTabBar = hasSplits

  return (
    <div className="flex h-full w-full flex-col">
      {showPaneTabBar && (
        <PaneTabBar
          items={paneGroup.items}
          activeItemIndex={activeIndex}
          onActivate={handleActivate}
          onClose={handleClose}
          onClosePane={hasSplits ? handleClosePane : undefined}
        />
      )}

      <div className="flex-1 min-h-0">
        {activeItem.type === 'terminal' && terminalData ? (
          <TerminalView
            terminalId={terminalData.terminalId}
            cwd={terminalData.cwd}
            command={terminalData.command}
            args={terminalData.args}
            onExit={() => handleClose(activeItem.id)}
          />
        ) : activeItem.type === 'file-viewer' && fileData ? (
          <FileViewerPane
            filePath={fileData.filePath}
            paneId={activeItem.id}
            tabId={tabId}
            onClose={() => handleClose(activeItem.id)}
          />
        ) : null}
      </div>
    </div>
  )
}

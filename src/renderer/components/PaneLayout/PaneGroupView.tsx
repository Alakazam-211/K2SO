import { useCallback, useState } from 'react'
import { AlacrittyTerminalView } from '@/components/Terminal/AlacrittyTerminalView'
import { FileViewerPane } from '@/components/FileViewerPane/FileViewerPane'
import { useTabsStore } from '@/stores/tabs'
import type { TerminalItemData, FileViewerItemData } from '@/stores/tabs'
import { useActiveAgentsStore, type ActiveAgent } from '@/stores/active-agents'
import AgentCloseDialog from '@/components/AgentCloseDialog/AgentCloseDialog'
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
  const removeTabFromGroup = useTabsStore((s) => s.removeTabFromGroup)

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

  const [pendingPaneClose, setPendingPaneClose] = useState<{
    itemId: string
    agents: ActiveAgent[]
  } | null>(null)

  const handleClose = useCallback(
    (itemId: string) => {
      const item = paneGroup?.items.find(i => i.id === itemId)
      if (item?.type === 'terminal') {
        const data = item.data as TerminalItemData
        const agent = useActiveAgentsStore.getState().agents.get(data.terminalId)
        if (agent) {
          setPendingPaneClose({ itemId, agents: [agent] })
          return
        }
      }
      closeItem?.(tabId, paneGroupId, itemId)
    },
    [tabId, paneGroupId, closeItem, paneGroup]
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
  // xterm.js removed — alacritty is the only backend

  // Only show per-pane tab bar when there are splits
  // (so the close-pane button is accessible).
  // Single-pane items are managed by the workspace TabBar at the top.
  const showPaneTabBar = hasSplits

  return (
    <>
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
              <AlacrittyTerminalView
                terminalId={terminalData.terminalId}
                tabId={tabId}
                cwd={terminalData.cwd}
                command={terminalData.command}
                args={terminalData.args}
                onExit={(exitCode) => {
                  if (exitCode === 127) {
                    const store = useTabsStore.getState()
                    const groupIdx = store.tabs.some((t) => t.id === tabId)
                      ? 0
                      : store.extraGroups.findIndex((g) => g.tabs.some((t) => t.id === tabId)) + 1
                    if (groupIdx >= 0) {
                      removeTabFromGroup(groupIdx, tabId)
                    }
                  } else if (exitCode === 0) {
                    handleClose(activeItem.id)
                  }
                }}
              />
          ) : activeItem.type === 'file-viewer' && fileData ? (
            <FileViewerPane
              filePath={fileData.filePath}
              mode={fileData.mode}
              paneId={activeItem.id}
              tabId={tabId}
              onClose={() => handleClose(activeItem.id)}
            />
          ) : null}
        </div>
      </div>

      {pendingPaneClose && (
        <AgentCloseDialog
          agents={pendingPaneClose.agents}
          mode="tab"
          onConfirm={() => {
            closeItem?.(tabId, paneGroupId, pendingPaneClose.itemId)
            setPendingPaneClose(null)
          }}
          onCancel={() => setPendingPaneClose(null)}
        />
      )}
    </>
  )
}

import { useCallback } from 'react'
import { Mosaic, MosaicWindow } from 'react-mosaic-component'
import type { MosaicBranch, MosaicNode } from 'react-mosaic-component'
import { TerminalView } from '@/components/Terminal/TerminalView'
import { FileViewerPane } from '@/components/FileViewerPane/FileViewerPane'
import { useTabsStore } from '@/stores/tabs'
import 'react-mosaic-component/react-mosaic-component.css'

interface PaneLayoutProps {
  tabId: string
}

export function PaneLayout({ tabId }: PaneLayoutProps): React.JSX.Element | null {
  const tab = useTabsStore((s) => s.tabs.find((t) => t.id === tabId))
  const updateMosaicTree = useTabsStore((s) => s.updateMosaicTree)
  const removePaneFromTab = useTabsStore((s) => s.removePaneFromTab)

  const handleChange = useCallback(
    (newTree: MosaicNode<string> | null) => {
      updateMosaicTree(tabId, newTree)
    },
    [tabId, updateMosaicTree]
  )

  if (!tab || tab.mosaicTree === null) {
    return (
      <div className="flex h-full w-full items-center justify-center text-[var(--color-text-muted)]">
        No terminal panes. Press Cmd+T to open a new tab.
      </div>
    )
  }

  const renderTile = (paneId: string, path: MosaicBranch[]): React.JSX.Element => {
    const pane = tab.panes.get(paneId)
    if (!pane) {
      return <div className="h-full w-full bg-[#0a0a0a]" />
    }

    return (
      <MosaicWindow<string>
        path={path}
        title=""
        toolbarControls={<></>}
        createNode={() => ''}
        renderPreview={() => <div />}
      >
        {pane.type === 'file-viewer' ? (
          <FileViewerPane
            filePath={pane.filePath}
            paneId={paneId}
            tabId={tabId}
            onClose={() => {
              removePaneFromTab(tabId, paneId)
            }}
          />
        ) : pane.type === 'terminal' ? (
          <TerminalView
            terminalId={pane.terminalId}
            cwd={pane.cwd}
            command={pane.command}
            args={pane.args}
            onExit={() => {
              removePaneFromTab(tabId, paneId)
            }}
          />
        ) : null}
      </MosaicWindow>
    )
  }

  return (
    <div className="mosaic-dark h-full w-full">
      <Mosaic<string>
        renderTile={renderTile}
        value={tab.mosaicTree}
        onChange={handleChange}
        className=""
      />
    </div>
  )
}

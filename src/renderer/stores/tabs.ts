import { create } from 'zustand'
import type { MosaicNode, MosaicDirection } from 'react-mosaic-component'

// ── Types ────────────────────────────────────────────────────────────────

export interface TerminalPaneData {
  type: 'terminal'
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
}

export interface FileViewerPaneData {
  type: 'file-viewer'
  filePath: string
  pinned: boolean
}

/** @deprecated Use FileViewerPaneData instead */
export interface MarkdownPaneData {
  type: 'markdown'
  filePath: string
}

export type PaneData = TerminalPaneData | FileViewerPaneData

// Keep backward-compat alias
export type TerminalPane = TerminalPaneData

export interface Tab {
  id: string
  title: string
  mosaicTree: MosaicNode<string> | null
  panes: Map<string, PaneData>
}

interface TabsState {
  tabs: Tab[]
  activeTabId: string | null
  addTab: (cwd: string) => string
  removeTab: (tabId: string) => void
  setActiveTab: (tabId: string) => void
  splitPane: (
    tabId: string,
    existingPaneId: string,
    newPaneId: string,
    newPane: TerminalPaneData,
    direction: MosaicDirection
  ) => void
  updateMosaicTree: (tabId: string, tree: MosaicNode<string> | null) => void
  reorderTabs: (fromIndex: number, toIndex: number) => void
  addPaneToTab: (tabId: string, paneId: string, pane: PaneData) => void
  removePaneFromTab: (tabId: string, paneId: string) => void
  getActiveTab: () => Tab | undefined
  openFileInPane: (tabId: string, filePath: string) => void
  pinPane: (tabId: string, paneId: string) => void
  unpinPane: (tabId: string, paneId: string) => void
  openFileInNewTab: (filePath: string) => void
  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, splitDirection?: 'row' | 'column') => void
}

// ── Helpers ──────────────────────────────────────────────────────────────

let tabCounter = 0

function removePaneFromTree(
  tree: MosaicNode<string> | null,
  paneId: string
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return tree === paneId ? null : tree
  }

  const newFirst = removePaneFromTree(tree.first, paneId)
  const newSecond = removePaneFromTree(tree.second, paneId)

  if (newFirst === null && newSecond === null) return null
  if (newFirst === null) return newSecond
  if (newSecond === null) return newFirst

  return { ...tree, first: newFirst, second: newSecond }
}

function getFirstLeaf(tree: MosaicNode<string> | null): string | null {
  if (tree === null) return null
  if (typeof tree === 'string') return tree
  return getFirstLeaf(tree.first)
}

// ── Store ────────────────────────────────────────────────────────────────

export const useTabsStore = create<TabsState>((set, get) => ({
  tabs: [],
  activeTabId: null,

  addTab: (cwd: string) => {
    tabCounter++
    const tabId = crypto.randomUUID()
    const paneId = crypto.randomUUID()

    const pane: TerminalPaneData = { type: 'terminal', terminalId: paneId, cwd }

    const tab: Tab = {
      id: tabId,
      title: `Terminal ${tabCounter}`,
      mosaicTree: paneId,
      panes: new Map([[paneId, pane]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))

    return paneId
  },

  removeTab: (tabId: string) => {
    set((state) => {
      const newTabs = state.tabs.filter((t) => t.id !== tabId)
      let newActiveId = state.activeTabId

      if (state.activeTabId === tabId) {
        const idx = state.tabs.findIndex((t) => t.id === tabId)
        if (newTabs.length > 0) {
          newActiveId = newTabs[Math.min(idx, newTabs.length - 1)].id
        } else {
          newActiveId = null
        }
      }

      return { tabs: newTabs, activeTabId: newActiveId }
    })
  },

  setActiveTab: (tabId: string) => {
    set({ activeTabId: tabId })
  },

  splitPane: (tabId, existingPaneId, newPaneId, newPane, direction) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const newPanes = new Map(tab.panes)
        newPanes.set(newPaneId, newPane)

        const newTree: MosaicNode<string> = {
          direction,
          first: existingPaneId,
          second: newPaneId,
          splitPercentage: 50
        }

        // Replace the existing pane in the tree with the split
        const updatedTree = replaceInTree(tab.mosaicTree, existingPaneId, newTree)

        return { ...tab, mosaicTree: updatedTree, panes: newPanes }
      })

      return { tabs }
    })
  },

  updateMosaicTree: (tabId, tree) => {
    set((state) => ({
      tabs: state.tabs.map((tab) =>
        tab.id === tabId ? { ...tab, mosaicTree: tree } : tab
      )
    }))
  },

  reorderTabs: (fromIndex, toIndex) => {
    set((state) => {
      const tabs = [...state.tabs]
      const [moved] = tabs.splice(fromIndex, 1)
      tabs.splice(toIndex, 0, moved)
      return { tabs }
    })
  },

  addPaneToTab: (tabId, paneId, pane) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPanes = new Map(tab.panes)
        newPanes.set(paneId, pane)
        return { ...tab, panes: newPanes }
      })
    }))
  },

  removePaneFromTab: (tabId, paneId) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPanes = new Map(tab.panes)
        newPanes.delete(paneId)
        const newTree = removePaneFromTree(tab.mosaicTree, paneId)
        return { ...tab, panes: newPanes, mosaicTree: newTree }
      })
    }))
  },

  getActiveTab: () => {
    const state = get()
    return state.tabs.find((t) => t.id === state.activeTabId)
  },

  openFileInPane: (tabId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        // Look for an existing unpinned file-viewer pane
        let unpinnedPaneId: string | null = null
        for (const [id, pane] of tab.panes) {
          if (pane.type === 'file-viewer' && !pane.pinned) {
            unpinnedPaneId = id
            break
          }
        }

        if (unpinnedPaneId) {
          // Reuse the unpinned pane — just update its filePath
          const newPanes = new Map(tab.panes)
          newPanes.set(unpinnedPaneId, { type: 'file-viewer', filePath, pinned: false })
          return { ...tab, panes: newPanes }
        }

        // No unpinned file-viewer pane found — create a new one
        const newPaneId = crypto.randomUUID()
        const filePane: FileViewerPaneData = { type: 'file-viewer', filePath, pinned: false }

        const newPanes = new Map(tab.panes)
        newPanes.set(newPaneId, filePane)

        // Split alongside the first existing leaf
        const existingLeaf = getFirstLeaf(tab.mosaicTree)

        let newTree: MosaicNode<string>
        if (existingLeaf && tab.mosaicTree !== null) {
          const splitNode: MosaicNode<string> = {
            direction: 'row',
            first: existingLeaf,
            second: newPaneId,
            splitPercentage: 50
          }
          newTree = replaceInTree(tab.mosaicTree, existingLeaf, splitNode) as MosaicNode<string>
        } else {
          newTree = newPaneId
        }

        return { ...tab, mosaicTree: newTree, panes: newPanes }
      })

      return { tabs }
    })
  },

  pinPane: (tabId: string, paneId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pane = tab.panes.get(paneId)
        if (!pane || pane.type !== 'file-viewer') return tab
        const newPanes = new Map(tab.panes)
        newPanes.set(paneId, { ...pane, pinned: true })
        return { ...tab, panes: newPanes }
      })
    }))
  },

  unpinPane: (tabId: string, paneId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pane = tab.panes.get(paneId)
        if (!pane || pane.type !== 'file-viewer') return tab
        const newPanes = new Map(tab.panes)
        newPanes.set(paneId, { ...pane, pinned: false })
        return { ...tab, panes: newPanes }
      })
    }))
  },

  openFileInNewTab: (filePath: string) => {
    const paneId = crypto.randomUUID()
    const tabId = crypto.randomUUID()
    const fileName = filePath.split('/').pop() || 'File'

    const pane: FileViewerPaneData = { type: 'file-viewer', filePath, pinned: true }

    const tab: Tab = {
      id: tabId,
      title: fileName,
      mosaicTree: paneId,
      panes: new Map([[paneId, pane]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))
  },

  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, _splitDirection: 'row' | 'column' = 'row') => {
    get().openFileInPane(tabId, filePath)
  }
}))

// ── Tree utilities ───────────────────────────────────────────────────────

function replaceInTree(
  tree: MosaicNode<string> | null,
  targetId: string,
  replacement: MosaicNode<string>
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return tree === targetId ? replacement : tree
  }
  return {
    ...tree,
    first: replaceInTree(tree.first, targetId, replacement) as MosaicNode<string>,
    second: replaceInTree(tree.second, targetId, replacement) as MosaicNode<string>
  }
}

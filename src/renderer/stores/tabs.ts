import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { MosaicNode, MosaicDirection } from 'react-mosaic-component'
import { RESUMABLE_CLI_TOOLS } from '@shared/constants'
import { useSettingsStore } from '@/stores/settings'

// Lazy reference to presets store — avoids circular dependency (presets → tabs → presets).
// Set by presets.ts on init via registerPresetsStore().
let _presetsStoreRef: (() => { presets: any[] }) | null = null
export function registerPresetsStore(getter: () => { presets: any[] }): void {
  _presetsStoreRef = getter
}

// ── Cross-window tab sync ────────────────────────────────────────────────

export const WINDOW_ID = crypto.randomUUID() // unique per window instance

interface TabSyncPayload {
  windowId: string
  action: 'add' | 'remove' | 'title'
  groupIndex: number
  tabId: string
  title?: string
  terminalId?: string
  cwd?: string
  command?: string
  args?: string[]
}

function broadcastTabChange(payload: TabSyncPayload): void {
  invoke('broadcast_sync', {
    channel: 'sync:tabs',
    payload,
  }).catch((e) => console.warn('[tabs] broadcast failed:', e))
}

// ── Item Types ────────────────────────────────────────────────────────────

export interface TerminalItemData {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  sessionId?: string  // CLI tool session ID for resume on restart
}

export interface FileViewerItemData {
  filePath: string
  /** When 'diff', shows unified diff view instead of editor */
  mode?: 'edit' | 'diff'
  /** Saved scroll position (pixels) — restored on tab re-activation */
  scrollTop?: number
  /** Saved cursor position in the editor (character offset) */
  cursorPos?: number
}

export interface AgentItemData {
  agentName: string
  projectPath: string
}

export interface Item {
  id: string
  type: 'terminal' | 'file-viewer' | 'agent'
  data: TerminalItemData | FileViewerItemData | AgentItemData
  pinned?: boolean
}

// ── PaneGroup ─────────────────────────────────────────────────────────────

export interface PaneGroup {
  id: string
  items: Item[]
  activeItemIndex: number
}

// ── Legacy compat types (re-exported for consumers) ──────────────────────

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

export type PaneData = TerminalPaneData | FileViewerPaneData | { type: 'agent'; agentName: string; projectPath: string }

// Keep backward-compat alias
export type TerminalPane = TerminalPaneData

// ── Serialization Types ─────────────────────────────────────────────────

export interface SerializedItem {
  id: string
  type: 'terminal' | 'file-viewer' | 'agent'
  cwd?: string
  command?: string
  args?: string[]
  sessionId?: string  // CLI tool session ID for resume on restart
  filePath?: string
  pinned?: boolean
  agentName?: string
  projectPath?: string
}

export interface SerializedPaneGroup {
  id: string
  items: SerializedItem[]
  activeItemIndex: number
}

export interface SerializedTab {
  id: string
  title: string
  mosaicTree: MosaicNode<string> | null
  paneGroups: Record<string, SerializedPaneGroup>
  isSystemAgent?: boolean
}

export interface SerializedLayout {
  tabs: SerializedTab[]
  activeTabId: string | null
  extraGroups?: Array<{ tabs: SerializedTab[], activeTabId: string | null }>
  splitCount?: number
  activeGroupIndex?: number
}

// ── Background workspace snapshot (live tabs with running PTYs) ─────────

export interface WorkspaceTabSnapshot {
  tabs: Tab[]
  extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }>
  splitCount: number
  activeGroupIndex: number
  activeTabId: string | null
}

// ── Tab ─────────────────────────────────────────────────────────────────

export interface Tab {
  id: string
  title: string
  mosaicTree: MosaicNode<string> | null  // leaf strings = paneGroupIds
  paneGroups: Map<string, PaneGroup>
  isDirty?: boolean
  /** System agent tab — pinned at start of tab bar, can't be closed or reordered */
  isSystemAgent?: boolean
}

interface TabsState {
  tabs: Tab[]
  activeTabId: string | null

  // Existing actions (signatures preserved)
  addTab: (cwd: string, options?: { title?: string; command?: string; args?: string[] }) => string
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
  reorderTabs: (fromIndex: number, toIndex: number, groupIndex?: number) => void
  addPaneToTab: (tabId: string, paneId: string, pane: PaneData) => void
  removePaneFromTab: (tabId: string, paneGroupId: string) => void
  moveItemBetweenPanes: (fromTabId: string, fromPaneGroupId: string, itemId: string, toTabId: string, toPaneGroupId: string) => void
  getActiveTab: () => Tab | undefined
  openFileInPane: (tabId: string, filePath: string) => void
  openAgentPane: (agentName: string, projectPath: string, title?: string) => void
  openFileAsTab: (filePath: string) => void
  openFileInPaneGroup: (tabId: string, paneGroupId: string, filePath: string) => void
  openDiffInPane: (tabId: string, filePath: string) => void
  pinPane: (tabId: string, paneGroupId: string) => void
  unpinPane: (tabId: string, paneGroupId: string) => void
  openFileInNewTab: (filePath: string) => void
  openUntitledDocument: (cwd: string) => void
  setTabTitle: (tabId: string, title: string) => void
  renameTabByTitle: (oldTitle: string, newTitle: string) => void
  setTabDirty: (tabId: string, dirty: boolean) => void
  setFileViewerState: (tabId: string, paneId: string, itemId: string, state: { scrollTop?: number; cursorPos?: number }) => void
  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, splitDirection?: 'row' | 'column') => void

  // NEW: PaneGroup item management
  addItemToPaneGroup: (tabId: string, paneGroupId: string, item: Item) => void
  activateItemInPaneGroup: (tabId: string, paneGroupId: string, itemIndex: number) => void
  closeItemInPaneGroup: (tabId: string, paneGroupId: string, itemId: string) => void
  getActivePaneGroupId: (tabId: string) => string | null

  // Split the active tab's panes (up to max panes within a tab)
  splitActivePane: (cwd: string, maxPanes?: number) => boolean

  // Get the number of panes in the active tab
  getActivePaneCount: () => number

  // ── Tab Groups (independent columns, each with own tab bar) ──────
  splitCount: number  // 1, 2, or 3 columns
  extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }>
  activeGroupIndex: number  // which group receives new tabs by default

  splitTerminalArea: (cwd: string) => void    // add a column (max 3)
  unsplitTerminalArea: () => void              // remove rightmost column
  setActiveGroup: (index: number) => void
  addTabToGroup: (groupIndex: number, cwd: string, options?: { title?: string; command?: string; args?: string[] }) => string
  removeTabFromGroup: (groupIndex: number, tabId: string) => void
  setActiveTabInGroup: (groupIndex: number, tabId: string) => void
  moveTabToGroup: (fromGroup: number, toGroup: number, tabId: string) => void
  getGroupTabs: (groupIndex: number) => { tabs: Tab[], activeTabId: string | null }

  // Navigation history (back/forward)
  navHistory: string[]     // stack of tabIds
  navIndex: number         // current position (-1 = no history)
  canGoBack: () => boolean
  canGoForward: () => boolean
  goBack: () => void
  goForward: () => void

  // Layout persistence per workspace
  activeWorkspaceKey: string | null  // "projectId:workspaceId" for auto-save
  backgroundWorkspaces: Record<string, WorkspaceTabSnapshot>
  workspaceLayouts: Record<string, SerializedLayout>
  serializeCurrentLayout: () => SerializedLayout
  restoreLayout: (layout: SerializedLayout, cwd: string) => void
  saveLayoutForWorkspace: (projectId: string, workspaceId: string) => void
  loadLayoutForWorkspace: (projectId: string, workspaceId: string, cwd: string) => void
  loadWorkspaceSessionsFromDb: () => Promise<void>
  /** @deprecated Use loadWorkspaceSessionsFromDb instead */
  loadWorkspaceLayoutsFromSettings: () => Promise<void>
  clearAllTabs: () => void
  detectAndSaveSessionIds: () => Promise<void>

  // Background workspace management
  launchDefaultAgent: (key: string, cwd: string) => void
  stashWorkspace: (key: string) => void
  restoreWorkspace: (key: string, cwd: string) => void
  serializeAllWorkspaces: (activeKey: string) => Promise<void>
  clearBackgroundWorkspace: (key: string) => void
  persistActiveWorkspace: () => void
  /** Add a tab to a workspace without switching to it. If the workspace is active,
   *  adds directly. If background/stashed, saves to DB session so it's there when restored.
   *  Returns the terminal ID (paneGroupId) so the caller can spawn a background PTY. */
  addTabToWorkspace: (workspaceKey: string, cwd: string, options: { title: string; command: string; args: string[] }) => string | null

  // Pinned system agent tab
  /** Ensure a pinned agent tab exists for this workspace. Creates one if missing.
   *  Returns the tab ID. No-op if tab already exists. */
  ensureSystemAgentTab: (agentName: string, projectPath: string, title: string) => string
  /** Remove the pinned system agent tab (when agent mode is turned off). */
  removeSystemAgentTab: () => void
  /** Activate the pinned system agent tab (switches to it). */
  activateSystemAgentTab: () => void
  /** Get the pinned system agent tab if it exists. */
  getSystemAgentTab: () => Tab | undefined

  // Cross-window sync
  applyRemoteTabChange: (payload: TabSyncPayload) => void
  broadcastAllTabs: () => void
}

// ── Helpers ──────────────────────────────────────────────────────────────

let tabCounter = 0

/** Ensure the pinned system agent tab matches the given agent mode.
 *  Called by the projects store after workspace restore/switch.
 *  Resolves the primary agent name from the backend asynchronously. */
export function ensurePinnedAgentTabForMode(
  agentMode: string,
  projectPath: string,
): void {
  setTimeout(async () => {
    const tabsStore = useTabsStore.getState()
    if (!agentMode || agentMode === 'off') {
      tabsStore.removeSystemAgentTab()
      return
    }

    let title = 'Agent'
    if (agentMode === 'manager' || agentMode === 'coordinator') {
      title = 'Manager'
    } else if (agentMode === 'custom') {
      title = 'Agent'
    } else if (agentMode === 'agent') {
      title = 'K2SO'
    }

    // Resolve the actual primary agent name from the backend
    let agentName = '__lead__'
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const agents = await invoke<Array<{ name: string; isManager: boolean; agentType: string }>>('k2so_agents_list', { projectPath })
      if (agents && agents.length > 0) {
        // Find the primary agent: manager/coordinator first, then first custom, then first agent
        const manager = agents.find((a) => a.isManager || a.agentType === 'manager' || a.agentType === 'coordinator')
        const custom = agents.find((a) => a.agentType === 'custom')
        const k2so = agents.find((a) => a.agentType === 'k2so')
        if (agentMode === 'manager' || agentMode === 'coordinator') {
          agentName = manager?.name ?? agents[0].name
        } else if (agentMode === 'custom') {
          agentName = custom?.name ?? agents[0].name
        } else if (agentMode === 'agent') {
          agentName = k2so?.name ?? agents[0].name
        }
      }
    } catch {
      // Fall back to __lead__ if agent list fails
    }

    tabsStore.ensureSystemAgentTab(agentName, projectPath, title)
  }, 0)
}

/** Serialize a single Tab to SerializedTab (used by serializeCurrentLayout and serializeAllWorkspaces). */
function serializeTab(tab: Tab): SerializedTab {
  const paneGroupsObj: Record<string, SerializedPaneGroup> = {}
  for (const [pgId, pg] of tab.paneGroups) {
    const serializedItems: SerializedItem[] = pg.items.map((item) => {
      if (item.type === 'terminal') {
        const d = item.data as TerminalItemData
        return {
          id: item.id,
          type: 'terminal' as const,
          cwd: d.cwd,
          command: d.command,
          args: d.args,
          sessionId: d.sessionId,
        }
      } else if (item.type === 'agent') {
        const d = item.data as AgentItemData
        return {
          id: item.id,
          type: 'agent' as const,
          agentName: d.agentName,
          projectPath: d.projectPath,
        }
      } else {
        const d = item.data as FileViewerItemData
        return {
          id: item.id,
          type: 'file-viewer' as const,
          filePath: d.filePath,
          pinned: item.pinned,
        }
      }
    })
    paneGroupsObj[pgId] = {
      id: pg.id,
      items: serializedItems,
      activeItemIndex: Math.min(pg.activeItemIndex, Math.max(0, pg.items.length - 1)),
    }
  }
  return {
    id: tab.id,
    title: tab.title,
    mosaicTree: tab.mosaicTree,
    paneGroups: paneGroupsObj,
    ...(tab.isSystemAgent ? { isSystemAgent: true } : {}),
  }
}

/** Serialize a WorkspaceTabSnapshot (background workspace with live tabs). */
function serializeSnapshot(snapshot: WorkspaceTabSnapshot): SerializedLayout {
  const serializedExtraGroups = snapshot.extraGroups.map((group) => ({
    tabs: group.tabs.map(serializeTab),
    activeTabId: group.activeTabId,
  }))
  return {
    tabs: snapshot.tabs.map(serializeTab),
    activeTabId: snapshot.activeTabId,
    extraGroups: serializedExtraGroups.length > 0 ? serializedExtraGroups : undefined,
    splitCount: snapshot.splitCount > 1 ? snapshot.splitCount : undefined,
    activeGroupIndex: snapshot.activeGroupIndex > 0 ? snapshot.activeGroupIndex : undefined,
  }
}

/** Debounce timer for auto-saving the active workspace. */
let persistDebounceTimer: ReturnType<typeof setTimeout> | null = null

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

/** Count leaf nodes (panes) in a mosaic tree */
function countLeaves(tree: MosaicNode<string> | null): number {
  if (tree === null) return 0
  if (typeof tree === 'string') return 1
  return countLeaves(tree.first) + countLeaves(tree.second)
}

/** Find a tab across all groups (group 0 = main tabs, groups 1+ = extraGroups) */
function findTabAcrossGroups(state: { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> }, tabId: string): Tab | undefined {
  const found = state.tabs.find((t) => t.id === tabId)
  if (found) return found
  for (const group of state.extraGroups) {
    const f = group.tabs.find((t) => t.id === tabId)
    if (f) return f
  }
  return undefined
}

/** Apply a mapping function to a tab wherever it lives (group 0 or extraGroups) */
function mapTabAcrossGroups(
  state: { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> },
  tabId: string,
  fn: (tab: Tab) => Tab
): { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> } {
  // Check group 0
  const idx0 = state.tabs.findIndex((t) => t.id === tabId)
  if (idx0 >= 0) {
    return {
      tabs: state.tabs.map((t) => t.id === tabId ? fn(t) : t),
      extraGroups: state.extraGroups
    }
  }
  // Check extra groups
  for (let gi = 0; gi < state.extraGroups.length; gi++) {
    const idx = state.extraGroups[gi].tabs.findIndex((t) => t.id === tabId)
    if (idx >= 0) {
      const newGroups = [...state.extraGroups]
      newGroups[gi] = {
        ...newGroups[gi],
        tabs: newGroups[gi].tabs.map((t) => t.id === tabId ? fn(t) : t)
      }
      return { tabs: state.tabs, extraGroups: newGroups }
    }
  }
  return { tabs: state.tabs, extraGroups: state.extraGroups }
}

/** Create a PaneGroup with a single terminal item */
function makeTerminalPaneGroup(
  paneGroupId: string,
  cwd: string,
  options?: { command?: string; args?: string[] }
): PaneGroup {
  const itemId = crypto.randomUUID()
  return {
    id: paneGroupId,
    items: [
      {
        id: itemId,
        type: 'terminal',
        data: {
          terminalId: paneGroupId, // use paneGroupId as terminalId for compat
          cwd,
          command: options?.command,
          args: options?.args,
        },
      },
    ],
    activeItemIndex: 0,
  }
}

/** Create a PaneGroup with a single file-viewer item */
function makeFileViewerPaneGroup(
  paneGroupId: string,
  filePath: string,
  pinned: boolean
): PaneGroup {
  const itemId = crypto.randomUUID()
  return {
    id: paneGroupId,
    items: [
      {
        id: itemId,
        type: 'file-viewer',
        data: { filePath },
        pinned,
      },
    ],
    activeItemIndex: 0,
  }
}

/** Convert a PaneData to an Item (for backward compat in addPaneToTab) */
function paneDataToItem(pane: PaneData): Item {
  if (pane.type === 'terminal') {
    return {
      id: crypto.randomUUID(),
      type: 'terminal',
      data: {
        terminalId: pane.terminalId,
        cwd: pane.cwd,
        command: pane.command,
        args: pane.args,
      },
    }
  } else if (pane.type === 'agent') {
    return {
      id: crypto.randomUUID(),
      type: 'agent',
      data: { agentName: pane.agentName, projectPath: pane.projectPath },
    }
  } else {
    return {
      id: crypto.randomUUID(),
      type: 'file-viewer',
      data: { filePath: pane.filePath },
      pinned: pane.pinned,
    }
  }
}

/**
 * Get the active item of a PaneGroup projected as a flat PaneData.
 * Useful for backward-compat reads in consuming components.
 */
export function paneGroupToActivePaneData(pg: PaneGroup): PaneData {
  const item = pg.items[pg.activeItemIndex] ?? pg.items[0]
  if (item.type === 'terminal') {
    const d = item.data as TerminalItemData
    return {
      type: 'terminal',
      terminalId: d.terminalId,
      cwd: d.cwd,
      command: d.command,
      args: d.args,
    }
  } else if (item.type === 'agent') {
    const d = item.data as AgentItemData
    return {
      type: 'agent',
      agentName: d.agentName,
      projectPath: d.projectPath,
    }
  } else {
    const d = item.data as FileViewerItemData
    return {
      type: 'file-viewer',
      filePath: d.filePath,
      pinned: item.pinned ?? false,
    }
  }
}

// ── Store ────────────────────────────────────────────────────────────────

export const useTabsStore = create<TabsState>((set, get) => ({
  tabs: [],
  activeTabId: null,
  splitCount: 1,
  extraGroups: [],
  activeGroupIndex: 0,
  navHistory: [],
  navIndex: -1,

  canGoBack: () => get().navIndex > 0,
  canGoForward: () => {
    const s = get()
    return s.navIndex < s.navHistory.length - 1
  },
  goBack: () => {
    const s = get()
    if (s.navIndex <= 0) return
    const newIndex = s.navIndex - 1
    const tabId = s.navHistory[newIndex]
    // Check the tab still exists
    const allTabs = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    if (!allTabs.find((t) => t.id === tabId)) return
    set({ activeTabId: tabId, navIndex: newIndex })
  },
  goForward: () => {
    const s = get()
    if (s.navIndex >= s.navHistory.length - 1) return
    const newIndex = s.navIndex + 1
    const tabId = s.navHistory[newIndex]
    const allTabs = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    if (!allTabs.find((t) => t.id === tabId)) return
    set({ activeTabId: tabId, navIndex: newIndex })
  },

  addTab: (cwd: string, options?: { title?: string; command?: string; args?: string[] }) => {
    // Route to the active group
    const activeGroup = get().activeGroupIndex
    if (activeGroup > 0) {
      return get().addTabToGroup(activeGroup, cwd, options)
    }

    tabCounter++
    const tabId = crypto.randomUUID()
    const paneGroupId = crypto.randomUUID()

    const paneGroup = makeTerminalPaneGroup(paneGroupId, cwd, options)

    // Use provided title, or derive from command name, or fallback to "Terminal N"
    const title = options?.title
      ?? (options?.command ? options.command.split('/').pop()?.split(' ')[0] ?? `Terminal ${tabCounter}` : `Terminal ${tabCounter}`)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, paneGroup]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))

    // Broadcast to other windows
    const termItem = paneGroup.items[0]
    const termData = termItem?.data as TerminalItemData
    broadcastTabChange({
      windowId: WINDOW_ID,
      action: 'add',
      groupIndex: 0,
      tabId,
      title,
      terminalId: termData?.terminalId,
      cwd,
      command: options?.command,
      args: options?.args,
    })

    return paneGroupId
  },

  removeTab: (tabId: string) => {
    // Never close the pinned system agent tab
    const tab = get().tabs.find((t) => t.id === tabId)
    if (tab?.isSystemAgent) return

    // Kill all PTYs in the removed tab (PTYs survive tab switches for persistence)
    if (tab) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
          }
        }
      }
    }

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

    // Broadcast removal to other windows
    broadcastTabChange({
      windowId: WINDOW_ID,
      action: 'remove',
      groupIndex: 0,
      tabId,
    })
  },

  setActiveTab: (tabId: string) => {
    const state = get()
    if (tabId !== state.activeTabId) {
      // Push to nav history (truncate any forward history)
      const history = state.navHistory.slice(0, state.navIndex + 1)
      if (state.activeTabId) history.push(state.activeTabId)
      set({ activeTabId: tabId, navHistory: history, navIndex: history.length })
    } else {
      set({ activeTabId: tabId })
    }
  },

  splitPane: (tabId, existingPaneGroupId, newPaneGroupId, newPane, direction) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const newPaneGroups = new Map(tab.paneGroups)
        // Create a new PaneGroup from the TerminalPaneData
        const pg: PaneGroup = {
          id: newPaneGroupId,
          items: [paneDataToItem(newPane)],
          activeItemIndex: 0,
        }
        // Override the terminal's terminalId to match paneGroupId for compat
        if (pg.items[0].type === 'terminal') {
          (pg.items[0].data as TerminalItemData).terminalId = newPaneGroupId
        }
        newPaneGroups.set(newPaneGroupId, pg)

        const newTree: MosaicNode<string> = {
          direction,
          first: existingPaneGroupId,
          second: newPaneGroupId,
          splitPercentage: 50
        }

        // Replace the existing paneGroup in the tree with the split
        const updatedTree = replaceInTree(tab.mosaicTree, existingPaneGroupId, newTree)

        return { ...tab, mosaicTree: updatedTree, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  updateMosaicTree: (tabId, tree) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, mosaicTree: tree }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  reorderTabs: (fromIndex, toIndex, groupIndex = 0) => {
    set((state) => {
      if (groupIndex === 0) {
        const tabs = [...state.tabs]
        // Don't reorder the pinned system agent tab (always at index 0)
        if (tabs[fromIndex]?.isSystemAgent || tabs[toIndex]?.isSystemAgent) return {}
        const [moved] = tabs.splice(fromIndex, 1)
        tabs.splice(toIndex, 0, moved)
        return { tabs }
      }
      const extraGroups = state.extraGroups.map((g: { tabs: Tab[], activeTabId: string | null }, i: number) => {
        if (i !== groupIndex - 1) return g
        const tabs = [...g.tabs]
        const [moved] = tabs.splice(fromIndex, 1)
        tabs.splice(toIndex, 0, moved)
        return { ...g, tabs }
      })
      return { extraGroups }
    })
  },

  addPaneToTab: (tabId, paneGroupId, pane) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        // Create a new PaneGroup containing the single item
        const pg: PaneGroup = {
          id: paneGroupId,
          items: [paneDataToItem(pane)],
          activeItemIndex: 0,
        }
        newPaneGroups.set(paneGroupId, pg)
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  removePaneFromTab: (tabId, paneGroupId) => {
    // Kill PTYs in the removed pane group
    const tab = get().tabs.find((t) => t.id === tabId)
    const pg = tab?.paneGroups.get(paneGroupId)
    if (pg) {
      for (const item of pg.items) {
        if (item.type === 'terminal') {
          const data = item.data as TerminalItemData
          invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
        }
      }
    }

    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.delete(paneGroupId)
        const newTree = removePaneFromTree(tab.mosaicTree, paneGroupId)
        return { ...tab, paneGroups: newPaneGroups, mosaicTree: newTree }
      })
    }))
  },

  moveItemBetweenPanes: (fromTabId, fromPaneGroupId, itemId, toTabId, toPaneGroupId) => {
    set((state) => {
      // Find the source item
      const allTabs = [...state.tabs, ...state.extraGroups.flatMap((g) => g.tabs)]
      const fromTab = allTabs.find((t) => t.id === fromTabId)
      const fromPg = fromTab?.paneGroups.get(fromPaneGroupId)
      if (!fromPg) return state

      const itemIdx = fromPg.items.findIndex((i) => i.id === itemId)
      if (itemIdx < 0) return state
      const item = fromPg.items[itemIdx]

      // Remove from source
      const newFromItems = fromPg.items.filter((_, i) => i !== itemIdx)
      const newFromActiveIdx = Math.min(fromPg.activeItemIndex, Math.max(0, newFromItems.length - 1))

      // Add to target
      const toTab = allTabs.find((t) => t.id === toTabId)
      const toPg = toTab?.paneGroups.get(toPaneGroupId)
      if (!toPg) return state

      const newToItems = [...toPg.items, item]

      const updateTab = (tab: Tab): Tab => {
        if (tab.id === fromTabId) {
          const newPg = new Map(tab.paneGroups)
          newPg.set(fromPaneGroupId, { ...fromPg, items: newFromItems, activeItemIndex: newFromActiveIdx })
          return { ...tab, paneGroups: newPg }
        }
        if (tab.id === toTabId) {
          const newPg = new Map(tab.paneGroups)
          newPg.set(toPaneGroupId, { ...toPg, items: newToItems, activeItemIndex: newToItems.length - 1 })
          return { ...tab, paneGroups: newPg }
        }
        return tab
      }

      return {
        tabs: state.tabs.map(updateTab),
        extraGroups: state.extraGroups.map((g) => ({
          ...g,
          tabs: g.tabs.map(updateTab),
        })),
      }
    })
  },

  getActiveTab: () => {
    const state = get()
    return state.tabs.find((t) => t.id === state.activeTabId)
  },

  openFileInPane: (tabId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        // Find the active (or first) paneGroup and add a file-viewer item to it
        const activePgId = getFirstLeaf(tab.mosaicTree)
        if (!activePgId) return tab

        const pg = tab.paneGroups.get(activePgId)
        if (!pg) return tab

        // Look for an existing unpinned file-viewer item in this paneGroup
        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)

        if (unpinnedIdx !== -1) {
          // Reuse the unpinned item — update its filePath
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = {
            ...newItems[unpinnedIdx],
            data: { filePath },
          }
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: unpinnedIdx,
          })
        } else {
          // Add a new file-viewer item to the paneGroup
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: { filePath },
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: newItems.length - 1,
          })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  openAgentPane: (agentName: string, projectPath: string, title?: string) => {
    const state = get()

    // If this is the primary agent (manager lead, k2so agent, or custom agent),
    // redirect to the pinned system agent tab if it exists
    // If a pinned system agent tab exists, redirect to it for the primary agent
    const sysTab = state.tabs.find((t) => t.isSystemAgent)
    if (sysTab) {
      // Check if this is the agent in the system tab, or a known primary name
      const sysAgentName = Array.from(sysTab.paneGroups.values())[0]?.items[0]?.type === 'agent'
        ? (Array.from(sysTab.paneGroups.values())[0].items[0].data as AgentItemData).agentName
        : null
      if (agentName === sysAgentName || agentName === '__lead__' || agentName === '__workspace__') {
        set({ activeTabId: sysTab.id })
        return
      }
    }

    // Check if a tab for this agent already exists — switch to it
    for (const tab of state.tabs) {
      for (const [, pg] of tab.paneGroups) {
        const match = pg.items.find(
          (item) => item.type === 'agent' && (item.data as AgentItemData).agentName === agentName
        )
        if (match) {
          set({ activeTabId: tab.id })
          return
        }
      }
    }

    // Create a new tab with a single agent pane
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const agentItem: Item = {
      id: crypto.randomUUID(),
      type: 'agent',
      data: { agentName, projectPath },
    }
    const pg: PaneGroup = {
      id: pgId,
      items: [agentItem],
      activeItemIndex: 0,
    }
    const tabTitle = title ?? (agentName === '__workspace__' ? 'Work Board' : agentName)
    const tab: Tab = {
      id: tabId,
      title: tabTitle,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]]),
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeTabId: tabId }))
  },

  ensureSystemAgentTab: (agentName: string, projectPath: string, title: string) => {
    const state = get()

    // Already exists? Return its ID
    const existing = state.tabs.find((t) => t.isSystemAgent)
    if (existing) return existing.id

    // Create pinned agent tab at position 0
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const agentItem: Item = {
      id: crypto.randomUUID(),
      type: 'agent',
      data: { agentName, projectPath },
    }
    const pg: PaneGroup = {
      id: pgId,
      items: [agentItem],
      activeItemIndex: 0,
    }
    const tab: Tab = {
      id: tabId,
      title: title,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]]),
      isSystemAgent: true,
    }
    // Insert at position 0 (pinned to left)
    set((s) => ({ tabs: [tab, ...s.tabs] }))
    return tabId
  },

  removeSystemAgentTab: () => {
    set((s) => ({ tabs: s.tabs.filter((t) => !t.isSystemAgent) }))
  },

  activateSystemAgentTab: () => {
    const state = get()
    const sysTab = state.tabs.find((t) => t.isSystemAgent)
    if (sysTab) {
      set({ activeTabId: sysTab.id })
    }
  },

  getSystemAgentTab: () => {
    return get().tabs.find((t) => t.isSystemAgent)
  },

  openFileAsTab: (filePath: string) => {
    const state = get()

    // Check if a tab for this file already exists — switch to it
    for (const tab of state.tabs) {
      for (const [, pg] of tab.paneGroups) {
        const match = pg.items.find(
          (item) => item.type === 'file-viewer' && (item.data as FileViewerItemData).filePath === filePath
        )
        if (match) {
          set({ activeTabId: tab.id })
          return
        }
      }
    }

    // Create a new tab with a file-viewer pane
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const title = filePath.split('/').pop() || filePath
    const pg = makeFileViewerPaneGroup(pgId, filePath, false)
    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]]),
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeTabId: tabId }))
  },

  openFileInPaneGroup: (tabId: string, paneGroupId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)

        if (unpinnedIdx !== -1) {
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = { ...newItems[unpinnedIdx], data: { filePath } }
          newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: unpinnedIdx })
        } else {
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: { filePath },
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newItems.length - 1 })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  openDiffInPane: (tabId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const activePgId = getFirstLeaf(tab.mosaicTree)
        if (!activePgId) return tab

        const pg = tab.paneGroups.get(activePgId)
        if (!pg) return tab

        // Look for an existing unpinned file-viewer item to reuse
        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)
        const diffData: FileViewerItemData = { filePath, mode: 'diff' }

        if (unpinnedIdx !== -1) {
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = {
            ...newItems[unpinnedIdx],
            data: diffData,
          }
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: unpinnedIdx,
          })
        } else {
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: diffData,
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: newItems.length - 1,
          })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  pinPane: (tabId: string, paneGroupId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        // Pin the active item if it's a file-viewer
        const activeItem = pg.items[pg.activeItemIndex]
        if (!activeItem || activeItem.type !== 'file-viewer') return tab

        const newItems = [...pg.items]
        newItems[pg.activeItemIndex] = { ...activeItem, pinned: true }
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  unpinPane: (tabId: string, paneGroupId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const activeItem = pg.items[pg.activeItemIndex]
        if (!activeItem || activeItem.type !== 'file-viewer') return tab

        const newItems = [...pg.items]
        newItems[pg.activeItemIndex] = { ...activeItem, pinned: false }
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  openFileInNewTab: (filePath: string) => {
    const paneGroupId = crypto.randomUUID()
    const tabId = crypto.randomUUID()
    const fileName = filePath.split('/').pop() || 'File'

    const pg = makeFileViewerPaneGroup(paneGroupId, filePath, true)

    const tab: Tab = {
      id: tabId,
      title: fileName,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, pg]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))
  },

  openUntitledDocument: (cwd: string) => {
    // Count existing untitled docs to generate a unique name
    const state = get()
    const allTabs = [
      ...state.tabs,
      ...state.extraGroups.flatMap((g: { tabs: Tab[] }) => g.tabs)
    ]
    const untitledPattern = /^Untitled(?:-(\d+))?$/
    let maxNum = 0
    for (const t of allTabs) {
      const m = untitledPattern.exec(t.title)
      if (m) maxNum = Math.max(maxNum, m[1] ? parseInt(m[1], 10) : 1)
    }
    const num = maxNum + 1
    const title = num === 1 ? 'Untitled' : `Untitled-${num}`

    const paneGroupId = crypto.randomUUID()
    const tabId = crypto.randomUUID()
    const filePath = `${cwd}/${title}`

    const pg = makeFileViewerPaneGroup(paneGroupId, filePath, true)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, pg]]),
      isDirty: true, // Mark as dirty since it's unsaved
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))
  },

  setTabTitle: (tabId: string, title: string) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, title }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
    broadcastTabChange({ windowId: WINDOW_ID, action: 'title', groupIndex: 0, tabId, title })
  },

  renameTabByTitle: (oldTitle: string, newTitle: string) => {
    set((state) => {
      const updateTab = (tab: any) =>
        tab.title === oldTitle ? { ...tab, title: newTitle } : tab
      return {
        tabs: state.tabs.map(updateTab),
        extraGroups: state.extraGroups.map((group: any) => ({
          ...group,
          tabs: group.tabs.map(updateTab),
        })),
      }
    })
  },

  setTabDirty: (tabId: string, dirty: boolean) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, isDirty: dirty }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  setFileViewerState: (tabId: string, paneId: string, itemId: string, viewerState: { scrollTop?: number; cursorPos?: number }) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneId)
        if (!pg) return tab
        const newItems = pg.items.map((item) => {
          if (item.id !== itemId || item.type !== 'file-viewer') return item
          const prevData = item.data as FileViewerItemData
          return { ...item, data: { ...prevData, ...viewerState } }
        })
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, _splitDirection: 'row' | 'column' = 'row') => {
    get().openFileInPane(tabId, filePath)
  },

  // ── NEW: PaneGroup item management ────────────────────────────────────

  addItemToPaneGroup: (tabId: string, paneGroupId: string, item: Item) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab
        const newItems = [...pg.items, item]
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newItems.length - 1 })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  activateItemInPaneGroup: (tabId: string, paneGroupId: string, itemIndex: number) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg || itemIndex < 0 || itemIndex >= pg.items.length) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, activeItemIndex: itemIndex })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  closeItemInPaneGroup: (tabId: string, paneGroupId: string, itemId: string) => {
    // Kill the PTY for the removed terminal item
    const tab = findTabAcrossGroups(get(), tabId)
    const pg = tab?.paneGroups.get(paneGroupId)
    const removedItem = pg?.items.find((item) => item.id === itemId)
    if (removedItem?.type === 'terminal') {
      const data = removedItem.data as TerminalItemData
      invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
    }

    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const newItems = pg.items.filter((item) => item.id !== itemId)

        if (newItems.length === 0) {
          const newPaneGroups = new Map(tab.paneGroups)
          newPaneGroups.delete(paneGroupId)
          const newTree = removePaneFromTree(tab.mosaicTree, paneGroupId)
          return { ...tab, paneGroups: newPaneGroups, mosaicTree: newTree }
        }

        const newActiveIndex = Math.min(pg.activeItemIndex, newItems.length - 1)
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newActiveIndex })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  getActivePaneGroupId: (tabId: string): string | null => {
    const state = get()
    const tab = findTabAcrossGroups(state, tabId)
    if (!tab) return null
    return getFirstLeaf(tab.mosaicTree)
  },

  splitActivePane: (cwd: string, maxPanes: number = 3): boolean => {
    const state = get()
    const tab = state.tabs.find((t) => t.id === state.activeTabId)
    if (!tab) return false

    const currentCount = countLeaves(tab.mosaicTree)
    if (currentCount >= maxPanes) return false

    const existingPaneId = getFirstLeaf(tab.mosaicTree)
    if (!existingPaneId) return false

    const newPaneId = crypto.randomUUID()
    const newPane: TerminalPaneData = {
      type: 'terminal',
      terminalId: newPaneId,
      cwd,
    }

    get().splitPane(tab.id, existingPaneId, newPaneId, newPane, 'row')
    return true
  },

  getActivePaneCount: (): number => {
    const state = get()
    const tab = state.tabs.find((t) => t.id === state.activeTabId)
    if (!tab) return 0
    return countLeaves(tab.mosaicTree)
  },

  // ── Tab Groups ──────────────────────────────────────────────────────

  splitTerminalArea: (cwd: string) => {
    const state = get()
    if (state.splitCount >= 3) return

    // Create a new group with a fresh terminal tab
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const pg = makeTerminalPaneGroup(pgId, cwd)
    const tab: Tab = {
      id: tabId,
      title: `Terminal ${tabCounter}`,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]])
    }

    const newGroups = [...state.extraGroups, { tabs: [tab], activeTabId: tabId }]
    set({
      splitCount: state.splitCount + 1,
      extraGroups: newGroups,
      activeGroupIndex: state.splitCount  // focus the new group
    })
  },

  unsplitTerminalArea: () => {
    const state = get()
    if (state.splitCount <= 1) return

    // Move tabs from the rightmost group into the group to its left
    // (don't kill PTYs — preserve all terminals)
    const removedGroup = state.extraGroups[state.extraGroups.length - 1]
    const removedTabs = removedGroup?.tabs ?? []

    if (state.splitCount === 2) {
      // Removing group 1 → merge its tabs into group 0
      set({
        tabs: [...state.tabs, ...removedTabs],
        splitCount: 1,
        extraGroups: [],
        activeGroupIndex: 0
      })
    } else {
      // Removing group 2 → merge its tabs into group 1
      const newGroups = [...state.extraGroups]
      const targetGroup = newGroups[0] // group 1 (index 0 in extraGroups)
      newGroups[0] = {
        tabs: [...targetGroup.tabs, ...removedTabs],
        activeTabId: targetGroup.activeTabId
      }
      newGroups.pop() // remove the last group
      set({
        splitCount: state.splitCount - 1,
        extraGroups: newGroups,
        activeGroupIndex: Math.min(state.activeGroupIndex, state.splitCount - 2)
      })
    }
  },

  setActiveGroup: (index: number) => {
    set({ activeGroupIndex: index })
  },

  addTabToGroup: (groupIndex: number, cwd: string, options?: { title?: string; command?: string; args?: string[] }): string => {
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const pg = makeTerminalPaneGroup(pgId, cwd, options)
    const title = options?.title
      ?? (options?.command ? options.command.split('/').pop()?.split(' ')[0] ?? `Terminal ${tabCounter}` : `Terminal ${tabCounter}`)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]])
    }

    if (groupIndex === 0) {
      // Group 0 = main tabs
      set((state) => ({ tabs: [...state.tabs, tab], activeTabId: tabId }))
    } else {
      set((state) => {
        const newGroups = [...state.extraGroups]
        const gi = groupIndex - 1
        if (gi >= 0 && gi < newGroups.length) {
          newGroups[gi] = {
            tabs: [...newGroups[gi].tabs, tab],
            activeTabId: tabId
          }
        }
        return { extraGroups: newGroups }
      })
    }

    // Broadcast to other windows
    const termItem = pg.items[0]
    const termData = termItem?.data as TerminalItemData
    broadcastTabChange({
      windowId: WINDOW_ID,
      action: 'add',
      groupIndex,
      tabId,
      title,
      terminalId: termData?.terminalId,
      cwd,
      command: options?.command,
      args: options?.args,
    })

    return pgId
  },

  removeTabFromGroup: (groupIndex: number, tabId: string) => {
    if (groupIndex === 0) {
      get().removeTab(tabId)
      return
    }

    const state = get()
    const gi = groupIndex - 1
    if (gi < 0 || gi >= state.extraGroups.length) return

    const group = state.extraGroups[gi]
    // Kill PTYs
    const tab = group.tabs.find((t) => t.id === tabId)
    if (tab) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            invoke('terminal_kill', { id: (item.data as TerminalItemData).terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
          }
        }
      }
    }

    const newTabs = group.tabs.filter((t) => t.id !== tabId)
    let newActiveId = group.activeTabId
    if (group.activeTabId === tabId) {
      const idx = group.tabs.findIndex((t) => t.id === tabId)
      newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
    }

    const newGroups = [...state.extraGroups]
    newGroups[gi] = { tabs: newTabs, activeTabId: newActiveId }
    set({ extraGroups: newGroups })
  },

  setActiveTabInGroup: (groupIndex: number, tabId: string) => {
    // Track in nav history
    const state = get()
    const currentActive = groupIndex === 0
      ? state.activeTabId
      : state.extraGroups[groupIndex - 1]?.activeTabId
    if (currentActive && currentActive !== tabId) {
      const history = state.navHistory.slice(0, state.navIndex + 1)
      history.push(currentActive)
      set({ navHistory: history, navIndex: history.length })
    }

    if (groupIndex === 0) {
      set({ activeTabId: tabId })
      return
    }
    set((state) => {
      const newGroups = [...state.extraGroups]
      const gi = groupIndex - 1
      if (gi >= 0 && gi < newGroups.length) {
        newGroups[gi] = { ...newGroups[gi], activeTabId: tabId }
      }
      return { extraGroups: newGroups }
    })
  },

  moveTabToGroup: (fromGroup: number, toGroup: number, tabId: string) => {
    if (fromGroup === toGroup) return
    const state = get()

    // Get the tab from the source group
    let tab: Tab | undefined
    if (fromGroup === 0) {
      tab = state.tabs.find((t) => t.id === tabId)
    } else {
      const gi = fromGroup - 1
      tab = state.extraGroups[gi]?.tabs.find((t) => t.id === tabId)
    }
    if (!tab) return

    // Remove from source (without killing PTYs — we're moving, not closing)
    if (fromGroup === 0) {
      const newTabs = state.tabs.filter((t) => t.id !== tabId)
      let newActiveId = state.activeTabId
      if (state.activeTabId === tabId) {
        const idx = state.tabs.findIndex((t) => t.id === tabId)
        newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
      }
      set({ tabs: newTabs, activeTabId: newActiveId })
    } else {
      const gi = fromGroup - 1
      const group = state.extraGroups[gi]
      if (!group) return
      const newTabs = group.tabs.filter((t) => t.id !== tabId)
      let newActiveId = group.activeTabId
      if (group.activeTabId === tabId) {
        const idx = group.tabs.findIndex((t) => t.id === tabId)
        newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
      }
      const newGroups = [...state.extraGroups]
      newGroups[gi] = { tabs: newTabs, activeTabId: newActiveId }
      set({ extraGroups: newGroups })
    }

    // Add to target group
    const updatedState = get()
    if (toGroup === 0) {
      set({ tabs: [...updatedState.tabs, tab], activeTabId: tab.id })
    } else {
      const gi = toGroup - 1
      const newGroups = [...updatedState.extraGroups]
      if (gi >= 0 && gi < newGroups.length) {
        newGroups[gi] = {
          tabs: [...newGroups[gi].tabs, tab],
          activeTabId: tab.id
        }
        set({ extraGroups: newGroups })
      }
    }
  },

  getGroupTabs: (groupIndex: number): { tabs: Tab[], activeTabId: string | null } => {
    const state = get()
    if (groupIndex === 0) return { tabs: state.tabs, activeTabId: state.activeTabId }
    const gi = groupIndex - 1
    if (gi >= 0 && gi < state.extraGroups.length) return state.extraGroups[gi]
    return { tabs: [], activeTabId: null }
  },

  // ── Layout persistence per workspace ──────────────────────────────────

  activeWorkspaceKey: null,
  backgroundWorkspaces: {},
  workspaceLayouts: {},

  serializeCurrentLayout: (): SerializedLayout => {
    const state = get()
    const serializedTabs = state.tabs.map(serializeTab)
    const serializedExtraGroups = state.extraGroups.map((group) => ({
      tabs: group.tabs.map(serializeTab),
      activeTabId: group.activeTabId,
    }))
    return {
      tabs: serializedTabs,
      activeTabId: state.activeTabId,
      extraGroups: serializedExtraGroups.length > 0 ? serializedExtraGroups : undefined,
      splitCount: state.splitCount > 1 ? state.splitCount : undefined,
      activeGroupIndex: state.activeGroupIndex > 0 ? state.activeGroupIndex : undefined,
    }
  },

  restoreLayout: (layout: SerializedLayout, cwd: string) => {
    try {
    const restoredTabs: Tab[] = layout.tabs.map((serializedTab) => {
      const paneGroups = new Map<string, PaneGroup>()

      // We need to remap paneGroup IDs: old serialized IDs -> new UUIDs
      // because terminal items need fresh terminalIds (old PTYs are dead).
      const idMap = new Map<string, string>()

      // Handle both new format (paneGroups) and legacy format (panes)
      const serializedPaneGroups = serializedTab.paneGroups
        ?? convertLegacyPanes((serializedTab as any).panes)

      if (!serializedPaneGroups || typeof serializedPaneGroups !== 'object') {
        console.warn('[tabs] Corrupted layout: missing paneGroups, creating fresh tab')
        const pgId = crypto.randomUUID()
        const pg = makeTerminalPaneGroup(pgId, cwd)
        paneGroups.set(pgId, pg)
        idMap.set('default', pgId)
      } else {
      for (const [oldPgId, serializedPg] of Object.entries(serializedPaneGroups)) {
        // Reuse the saved ID — if a background PTY was already spawned with it
        // (e.g. delegate), the backend skips re-creation and connects to the
        // existing process. If no PTY exists, a new one is created with this ID.
        const newPgId = oldPgId
        idMap.set(oldPgId, newPgId)

        const rawItems = Array.isArray(serializedPg?.items) ? serializedPg.items : []
        const items: Item[] = rawItems.map((si) => {
          if (si.type === 'terminal') {
            // If this terminal had a CLI tool session, restore with --resume
            let command = si.command
            let args = si.args
            const sessionId = si.sessionId

            if (sessionId && command) {
              const toolConfig = RESUMABLE_CLI_TOOLS[command]
              if (toolConfig) {
                // Preserve original args (e.g. --dangerously-skip-permissions) and append --resume
                const origArgs = (si.args ?? []).filter((a: string) => a !== toolConfig.resumeFlag && a !== sessionId)
                args = [...origArgs, toolConfig.resumeFlag, sessionId]
              }
            }

            return {
              id: crypto.randomUUID(),
              type: 'terminal' as const,
              data: {
                terminalId: newPgId,
                cwd: si.cwd ?? cwd,
                command,
                args,
                sessionId,
              },
            }
          } else if (si.type === 'agent') {
            return {
              id: crypto.randomUUID(),
              type: 'agent' as const,
              data: { agentName: si.agentName ?? '', projectPath: si.projectPath ?? cwd },
            }
          } else {
            return {
              id: crypto.randomUUID(),
              type: 'file-viewer' as const,
              data: { filePath: si.filePath ?? '' },
              pinned: si.pinned ?? false,
            }
          }
        })

        // Ensure at least one item per pane group
        if (items.length === 0) {
          items.push({
            id: crypto.randomUUID(),
            type: 'terminal',
            data: { terminalId: newPgId, cwd },
          })
        }

        const clampedIndex = Math.max(0, Math.min(serializedPg?.activeItemIndex ?? 0, items.length - 1))
        paneGroups.set(newPgId, {
          id: newPgId,
          items,
          activeItemIndex: clampedIndex,
        })
      }
      }

      // Remap the mosaic tree IDs
      const remappedTree = remapMosaicIds(serializedTab.mosaicTree, idMap)

      tabCounter++
      return {
        id: crypto.randomUUID(),
        title: serializedTab.title,
        mosaicTree: remappedTree,
        paneGroups,
        ...(serializedTab.isSystemAgent ? { isSystemAgent: true } : {}),
      }
    })

    // Restore activeTabId by index position (IDs are remapped)
    const activeTabIndex = layout.activeTabId
      ? layout.tabs.findIndex((t) => t.id === layout.activeTabId)
      : -1
    const restoredActiveTabId = activeTabIndex >= 0 && activeTabIndex < restoredTabs.length
      ? restoredTabs[activeTabIndex].id
      : restoredTabs.length > 0 ? restoredTabs[0].id : null

    // Restore extra groups (split columns)
    const restoredExtraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> = []
    if (layout.extraGroups) {
      for (const group of layout.extraGroups) {
        const groupTabs = group.tabs.map((serializedTab) => {
          // Use the same restore logic as group 0
          const paneGroups = new Map<string, PaneGroup>()
          const idMap = new Map<string, string>()
          const serializedPaneGroups = serializedTab.paneGroups
            ?? convertLegacyPanes((serializedTab as any).panes)
          if (serializedPaneGroups && typeof serializedPaneGroups === 'object') {
            for (const [oldPgId, serializedPg] of Object.entries(serializedPaneGroups)) {
              const newPgId = crypto.randomUUID()
              idMap.set(oldPgId, newPgId)
              const rawItems = Array.isArray(serializedPg?.items) ? serializedPg.items : []
              const items: Item[] = rawItems.map((si) => {
                if (si.type === 'terminal') {
                  let command = si.command
                  let args = si.args
                  const sessionId = si.sessionId
                  if (sessionId && command) {
                    const toolConfig = RESUMABLE_CLI_TOOLS[command]
                    if (toolConfig) {
                      const origArgs = (si.args ?? []).filter((a: string) => a !== toolConfig.resumeFlag && a !== sessionId)
                      args = [...origArgs, toolConfig.resumeFlag, sessionId]
                    }
                  }
                  return {
                    id: crypto.randomUUID(),
                    type: 'terminal' as const,
                    data: { terminalId: newPgId, cwd: si.cwd ?? cwd, command, args, sessionId },
                  }
                } else if (si.type === 'agent') {
                  return {
                    id: crypto.randomUUID(),
                    type: 'agent' as const,
                    data: { agentName: si.agentName ?? '', projectPath: si.projectPath ?? cwd },
                  }
                } else {
                  return {
                    id: crypto.randomUUID(),
                    type: 'file-viewer' as const,
                    data: { filePath: si.filePath ?? '' },
                    pinned: si.pinned ?? false,
                  }
                }
              })
              if (items.length === 0) {
                items.push({ id: crypto.randomUUID(), type: 'terminal', data: { terminalId: newPgId, cwd } })
              }
              const clampedIndex = Math.max(0, Math.min(serializedPg?.activeItemIndex ?? 0, items.length - 1))
              paneGroups.set(newPgId, { id: newPgId, items, activeItemIndex: clampedIndex })
            }
          }
          tabCounter++
          return {
            id: crypto.randomUUID(),
            title: serializedTab.title,
            mosaicTree: remapMosaicIds(serializedTab.mosaicTree, idMap),
            paneGroups,
          }
        })
        const groupActiveIndex = group.activeTabId
          ? group.tabs.findIndex((t) => t.id === group.activeTabId)
          : -1
        restoredExtraGroups.push({
          tabs: groupTabs,
          activeTabId: groupActiveIndex >= 0 && groupActiveIndex < groupTabs.length
            ? groupTabs[groupActiveIndex].id
            : groupTabs.length > 0 ? groupTabs[0].id : null,
        })
      }
    }

    set({
      tabs: restoredTabs,
      activeTabId: restoredActiveTabId,
      extraGroups: restoredExtraGroups,
      splitCount: layout.splitCount ?? 1,
      activeGroupIndex: layout.activeGroupIndex ?? 0,
    })
    } catch (err) {
      console.error('[tabs] Failed to restore layout, falling back to fresh tab:', err)
      tabCounter++
      const tabId = crypto.randomUUID()
      const paneGroupId = crypto.randomUUID()
      const pg = makeTerminalPaneGroup(paneGroupId, cwd)
      const tab: Tab = {
        id: tabId,
        title: `Terminal ${tabCounter}`,
        mosaicTree: paneGroupId,
        paneGroups: new Map([[paneGroupId, pg]]),
      }
      set({ tabs: [tab], activeTabId: tabId, extraGroups: [], splitCount: 1, activeGroupIndex: 0 })
    }
  },

  saveLayoutForWorkspace: (projectId: string, workspaceId: string) => {
    const state = get()
    const key = `${projectId}:${workspaceId}`

    if (state.tabs.length === 0) {
      // Save empty state so next open shows the empty workspace hints
      // instead of restoring a stale session
      const { [key]: _, ...remaining } = state.workspaceLayouts
      set({ workspaceLayouts: remaining })
      invoke('workspace_session_delete', { projectId, workspaceId }).catch(() => {})
      return
    }

    const layout = state.serializeCurrentLayout()
    set({ workspaceLayouts: { ...state.workspaceLayouts, [key]: layout } })

    // Persist to SQLite
    invoke('workspace_session_save', {
      projectId,
      workspaceId,
      layoutJson: JSON.stringify(layout),
    }).catch((err) => {
      console.error('[tabs] Failed to persist workspace layout:', err)
    })
  },

  loadLayoutForWorkspace: (projectId: string, workspaceId: string, cwd: string) => {
    const key = `${projectId}:${workspaceId}`
    const savedLayout = get().workspaceLayouts[key]
    // Kill any existing PTYs in the active view before restoring
    get().clearAllTabs()

    if (savedLayout && savedLayout.tabs && savedLayout.tabs.length > 0) {
      get().restoreLayout(savedLayout, cwd)
      set({ activeWorkspaceKey: key })
    } else {
      // Set key early so race-condition guards work
      set({ activeWorkspaceKey: key })

      // Try loading from DB
      invoke<string | null>('workspace_session_load', { projectId, workspaceId })
        .then((json) => {
          // Guard: bail if user already switched to a different workspace
          if (get().activeWorkspaceKey !== key) return

          if (json) {
            try {
              const layout = JSON.parse(json) as SerializedLayout
              if (layout.tabs && layout.tabs.length > 0) {
                set({ workspaceLayouts: { ...get().workspaceLayouts, [key]: layout } })
                get().restoreLayout(layout, cwd)
                return
              }
            } catch (err) {
              console.error('[tabs] Failed to parse DB layout:', err)
            }
          }
          // No saved layout — auto-launch default agent after short delay
          get().launchDefaultAgent(key, cwd)
        })
        .catch(() => {
          // DB unavailable — auto-launch default agent
          get().launchDefaultAgent(key, cwd)
        })
    }
  },

  loadWorkspaceSessionsFromDb: async () => {
    try {
      const sessions = await invoke<Array<{ projectId: string, workspaceId: string, layoutJson: string }>>('workspace_session_load_all')
      const layouts: Record<string, SerializedLayout> = {}
      for (const session of sessions) {
        try {
          layouts[`${session.projectId}:${session.workspaceId}`] = JSON.parse(session.layoutJson)
        } catch { /* skip corrupt entries */ }
      }
      set({ workspaceLayouts: layouts })
    } catch (err) {
      console.error('[tabs] Failed to load workspace sessions from DB:', err)
    }
  },

  /** @deprecated Use loadWorkspaceSessionsFromDb instead */
  loadWorkspaceLayoutsFromSettings: async () => {
    // Redirect to new DB-backed loader
    return get().loadWorkspaceSessionsFromDb()
  },

  clearAllTabs: () => {
    const state = get()
    // Kill all PTYs in group 0
    for (const tab of state.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
          }
        }
      }
    }
    // Kill all PTYs in extra groups
    for (const group of state.extraGroups) {
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const data = item.data as TerminalItemData
              invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
            }
          }
        }
      }
    }
    set({ tabs: [], activeTabId: null, extraGroups: [], splitCount: 1, activeGroupIndex: 0 })
  },

  // Detect active CLI tool session IDs for all running terminals
  // across active tabs, extra groups, and background workspaces.
  detectAndSaveSessionIds: async () => {
    const state = get()
    let updated = false

    // Helper to detect sessions in a list of tabs
    const detectInTabs = async (tabs: Tab[]): Promise<void> => {
      for (const tab of tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type !== 'terminal') continue
            const d = item.data as TerminalItemData
            if (!d.command || d.sessionId) continue
            const toolConfig = RESUMABLE_CLI_TOOLS[d.command]
            if (!toolConfig) continue
            try {
              const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
                provider: toolConfig.provider,
                projectPath: d.cwd,
              })
              if (sessionId) {
                ;(item.data as TerminalItemData).sessionId = sessionId
                updated = true
              }
            } catch (err) {
              console.error('[tabs] Failed to detect session for', d.command, err)
            }
          }
        }
      }
    }

    // Active group 0
    await detectInTabs(state.tabs)
    // Extra groups
    for (const group of state.extraGroups) {
      await detectInTabs(group.tabs)
    }
    // Background workspaces
    for (const snapshot of Object.values(state.backgroundWorkspaces)) {
      await detectInTabs(snapshot.tabs)
      for (const group of snapshot.extraGroups) {
        await detectInTabs(group.tabs)
      }
    }

    if (updated) {
      // Trigger re-render with shallow copies so serialization picks up mutations
      set({
        tabs: [...state.tabs],
        extraGroups: [...state.extraGroups],
        backgroundWorkspaces: { ...state.backgroundWorkspaces },
      })
    }
  },

  // ── Background workspace management ─────────────────────────────────

  launchDefaultAgent: (key: string, cwd: string) => {
    // Short delay (100ms) to let React finish the workspace switch render
    setTimeout(async () => {
      if (get().activeWorkspaceKey === key && get().tabs.length === 0) {
        tabCounter++
        const tabId = crypto.randomUUID()
        const paneGroupId = crypto.randomUUID()

        // Look up default agent preset
        let agentOpts: { command?: string; args?: string[]; title?: string } = {}
        try {
          const defaultAgent = useSettingsStore.getState().defaultAgent
          if (defaultAgent && _presetsStoreRef) {
            const presets = _presetsStoreRef().presets
            const preset = presets.find((p: any) => {
              const cmd = p.command.split(/\s+/)[0]
              return cmd === defaultAgent && p.enabled
            })
            if (preset) {
              const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
              const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
              agentOpts = { command: cleaned[0], args: cleaned.slice(1), title: preset.label }
            }
          }
        } catch { /* fall back to plain terminal */ }

        // Resume previous session if this is a resumable CLI tool (e.g. Claude)
        // This makes returning to K2SO/Custom agent workspaces feel like picking
        // up an ongoing conversation rather than starting fresh every time.
        if (agentOpts.command) {
          const toolConfig = RESUMABLE_CLI_TOOLS[agentOpts.command]
          if (toolConfig) {
            try {
              const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
                provider: toolConfig.provider,
                projectPath: cwd,
              })
              if (sessionId) {
                const baseArgs = agentOpts.args ?? []
                agentOpts.args = [...baseArgs, toolConfig.resumeFlag, sessionId]
              }
            } catch { /* session detection failed — launch fresh */ }
          }
        }

        const pg = makeTerminalPaneGroup(paneGroupId, cwd, agentOpts.command ? { command: agentOpts.command, args: agentOpts.args } : undefined)
        const tab: Tab = {
          id: tabId,
          title: agentOpts.title || `Terminal ${tabCounter}`,
          mosaicTree: paneGroupId,
          paneGroups: new Map([[paneGroupId, pg]])
        }
        set({ tabs: [tab], activeTabId: tabId })
      }
    }, 100)
  },

  stashWorkspace: (key: string) => {
    const state = get()
    if (state.tabs.length === 0 && state.extraGroups.length === 0) {
      // Workspace is empty — clear background snapshot AND DB session so
      // restoreWorkspace doesn't resurrect old tabs from either source
      const { [key]: _, ...remaining } = state.backgroundWorkspaces
      const { [key]: _layout, ...remainingLayouts } = state.workspaceLayouts
      set({
        backgroundWorkspaces: remaining,
        workspaceLayouts: remainingLayouts,
        activeWorkspaceKey: null,
      })
      // Delete saved session from DB so loadLayoutForWorkspace falls through to launchDefaultAgent
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        invoke('workspace_session_delete', { projectId, workspaceId }).catch(() => {})
      }
      return
    }

    // Move active tabs into background (PTYs stay alive)
    set({
      backgroundWorkspaces: {
        ...state.backgroundWorkspaces,
        [key]: {
          tabs: state.tabs,
          extraGroups: state.extraGroups,
          splitCount: state.splitCount,
          activeGroupIndex: state.activeGroupIndex,
          activeTabId: state.activeTabId,
        }
      },
      // Clear active view (React unmounts, but PTYs stay alive in backend)
      tabs: [],
      activeTabId: null,
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: null,
    })
  },

  restoreWorkspace: (key: string, cwd: string) => {
    const state = get()
    const live = state.backgroundWorkspaces[key]
    if (live && (live.tabs.length > 0 || live.extraGroups.length > 0)) {
      // Live tabs with running PTYs — swap them in
      const { [key]: _, ...remaining } = state.backgroundWorkspaces
      set({
        tabs: live.tabs,
        activeTabId: live.activeTabId,
        extraGroups: live.extraGroups,
        splitCount: live.splitCount,
        activeGroupIndex: live.activeGroupIndex,
        backgroundWorkspaces: remaining,
        activeWorkspaceKey: key,
      })
      // Pinned agent tab is ensured by the projects store after restoreWorkspace
      return
    }

    // Safety: if stash didn't run (e.g. activeWorkspaceId was null), the old
    // tabs might still be in the active view. Clear them before restoring so
    // the new workspace doesn't inherit the previous workspace's tabs.
    if (state.tabs.length > 0 || state.extraGroups.length > 0) {
      console.warn('[tabs] restoreWorkspace: clearing %d lingering tabs', state.tabs.length)
      get().clearAllTabs()
    }

    // No live tabs — fall back to serialized layout (creates new PTYs)
    const [projectId, workspaceId] = key.split(':')
    if (projectId && workspaceId) {
      get().loadLayoutForWorkspace(projectId, workspaceId, cwd)
    }
    // Pinned agent tab is ensured by the projects store after restoreWorkspace
  },

  serializeAllWorkspaces: async (activeKey: string) => {
    const state = get()

    // Serialize + save current active workspace
    if (state.tabs.length > 0 || state.extraGroups.length > 0) {
      const layout = state.serializeCurrentLayout()
      const [projectId, workspaceId] = activeKey.split(':')
      if (projectId && workspaceId) {
        await invoke('workspace_session_save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        }).catch((err) => console.error('[tabs] Failed to save active workspace:', err))
      }
    }

    // Serialize + save each background workspace
    for (const [key, snapshot] of Object.entries(state.backgroundWorkspaces)) {
      const layout = serializeSnapshot(snapshot)
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        await invoke('workspace_session_save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        }).catch((err) => console.error('[tabs] Failed to save background workspace:', key, err))
      }
    }
  },

  clearBackgroundWorkspace: (key: string) => {
    const state = get()
    const snapshot = state.backgroundWorkspaces[key]
    if (!snapshot) return

    // Kill all PTYs in the background workspace
    for (const tab of snapshot.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
          }
        }
      }
    }
    for (const group of snapshot.extraGroups) {
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const data = item.data as TerminalItemData
              invoke('terminal_kill', { id: data.terminalId }).catch((e) => console.warn('[tabs] terminal_kill failed:', e))
            }
          }
        }
      }
    }

    const { [key]: _, ...remaining } = state.backgroundWorkspaces
    set({ backgroundWorkspaces: remaining })
  },

  persistActiveWorkspace: () => {
    // Debounced save of the active workspace to DB
    if (persistDebounceTimer) clearTimeout(persistDebounceTimer)
    persistDebounceTimer = setTimeout(() => {
      const state = get()
      if (!state.activeWorkspaceKey) return
      if (state.tabs.length === 0 && state.extraGroups.length === 0) return

      const layout = state.serializeCurrentLayout()
      const [projectId, workspaceId] = state.activeWorkspaceKey.split(':')
      if (projectId && workspaceId) {
        invoke('workspace_session_save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        }).catch((err) => console.error('[tabs] Auto-save failed:', err))
      }
    }, 1000)
  },

  addTabToWorkspace: (workspaceKey: string, cwd: string, options: { title: string; command: string; args: string[] }): string | null => {
    const state = get()

    // If this IS the active workspace, just add the tab directly
    if (state.activeWorkspaceKey === workspaceKey) {
      state.addTabToGroup(0, cwd, options)
      return null // terminal created by the live tab
    }

    // Otherwise, save a session to the DB for this workspace so the tab
    // is waiting when the user navigates there. Build a minimal serialized layout.
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const terminalId = pgId

    const layout: SerializedLayout = {
      tabs: [{
        id: tabId,
        title: options.title,
        mosaicTree: pgId,
        paneGroups: {
          [pgId]: {
            id: pgId,
            items: [{
              id: crypto.randomUUID(),
              type: 'terminal',
              cwd,
              command: options.command,
              args: options.args,
            }],
            activeItemIndex: 0,
          }
        }
      }],
      activeTabId: tabId,
      splitCount: 1,
      activeGroupIndex: 0,
    }

    const [projectId, workspaceId] = workspaceKey.split(':')
    if (projectId && workspaceId) {
      invoke('workspace_session_save', {
        projectId,
        workspaceId,
        layoutJson: JSON.stringify(layout),
      }).catch((err) => console.error('[tabs] Failed to save agent tab to workspace:', err))
    }

    return pgId // terminal ID for background PTY spawning
  },

  applyRemoteTabChange: (payload: TabSyncPayload) => {
    // Ignore our own broadcasts
    if (payload.windowId === WINDOW_ID) return

    if (payload.action === 'add' && payload.terminalId && payload.cwd) {
      // Check if we already have this tab
      const existing = findTabAcrossGroups(get(), payload.tabId)
      if (existing) return

      tabCounter++
      const pgId = payload.terminalId // reuse the same terminal ID
      const pg = makeTerminalPaneGroup(pgId, payload.cwd, {
        command: payload.command,
        args: payload.args,
      })
      // Override the terminalId to match the source window's PTY
      if (pg.items[0]) {
        (pg.items[0].data as TerminalItemData).terminalId = payload.terminalId
      }

      const tab: Tab = {
        id: payload.tabId,
        title: payload.title ?? `Terminal ${tabCounter}`,
        mosaicTree: pgId,
        paneGroups: new Map([[pgId, pg]])
      }

      // Add to group 0 (main tabs) — the receiving window shows all synced tabs in main group
      set((state) => ({
        tabs: [...state.tabs, tab],
      }))
    } else if (payload.action === 'remove') {
      // Remove the tab without killing the PTY (the source window handles PTY lifecycle)
      const existing = findTabAcrossGroups(get(), payload.tabId)
      if (!existing) return

      set((state) => ({
        tabs: state.tabs.filter((t) => t.id !== payload.tabId),
        activeTabId: state.activeTabId === payload.tabId
          ? (state.tabs.find((t) => t.id !== payload.tabId)?.id ?? null)
          : state.activeTabId,
      }))
    } else if (payload.action === 'title') {
      set((state) => mapTabAcrossGroups(state, payload.tabId, (tab) => ({
        ...tab,
        title: payload.title ?? tab.title,
      })))
    }
  },

  broadcastAllTabs: () => {
    const state = get()
    // Broadcast all tabs from group 0
    for (const tab of state.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const d = item.data as TerminalItemData
            broadcastTabChange({
              windowId: WINDOW_ID,
              action: 'add',
              groupIndex: 0,
              tabId: tab.id,
              title: tab.title,
              terminalId: d.terminalId,
              cwd: d.cwd,
              command: d.command,
              args: d.args,
            })
          }
        }
      }
    }
    // Broadcast extra groups
    for (let gi = 0; gi < state.extraGroups.length; gi++) {
      for (const tab of state.extraGroups[gi].tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const d = item.data as TerminalItemData
              broadcastTabChange({
                windowId: WINDOW_ID,
                action: 'add',
                groupIndex: gi + 1,
                tabId: tab.id,
                title: tab.title,
                terminalId: d.terminalId,
                cwd: d.cwd,
                command: d.command,
                args: d.args,
              })
            }
          }
        }
      }
    }
  }
}))

// ── Tree utilities ───────────────────────────────────────────────────────

function remapMosaicIds(
  tree: MosaicNode<string> | null,
  idMap: Map<string, string>
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return idMap.get(tree) ?? tree
  }
  return {
    ...tree,
    first: remapMosaicIds(tree.first, idMap) as MosaicNode<string>,
    second: remapMosaicIds(tree.second, idMap) as MosaicNode<string>
  }
}

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

// ── Legacy format conversion ─────────────────────────────────────────────

interface LegacySerializedPaneData {
  type: 'terminal' | 'file-viewer'
  cwd?: string
  command?: string
  args?: string[]
  filePath?: string
  pinned?: boolean
}

/**
 * Convert legacy serialized panes (flat Record<string, PaneData>)
 * into the new SerializedPaneGroup format for backward-compat restore.
 */
function convertLegacyPanes(
  panes: Record<string, LegacySerializedPaneData> | undefined
): Record<string, SerializedPaneGroup> {
  if (!panes) return {}

  const result: Record<string, SerializedPaneGroup> = {}
  for (const [id, pane] of Object.entries(panes)) {
    const item: SerializedItem = {
      id,
      type: pane.type,
      cwd: pane.cwd,
      command: pane.command,
      args: pane.args,
      filePath: pane.filePath,
      pinned: pane.pinned,
    }
    result[id] = {
      id,
      items: [item],
      activeItemIndex: 0,
    }
  }
  return result
}

// ── Workspace ops event payloads ─────────────────────────────────────────

interface WsSplitPanePayload {
  tabId: string
  paneId: string
  direction: 'horizontal' | 'vertical'
}

interface WsClosePanePayload {
  tabId: string
  paneId: string
}

interface WsOpenDocumentPayload {
  tabId: string
  paneId: string
  filePath: string
}

interface WsOpenTerminalPayload {
  tabId: string
  paneId: string
  cwd: string
  command?: string
}

interface WsNewTabPayload {
  cwd: string
}

interface WsCloseTabPayload {
  tabId: string
}

interface LayoutLeafDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
  cwd?: string
}

interface LayoutBranchDescriptor {
  direction: 'horizontal' | 'vertical'
  children: [LayoutDescriptor, LayoutDescriptor]
  splitPercentage?: number
}

type LayoutDescriptor = LayoutLeafDescriptor | LayoutBranchDescriptor

function isLayoutBranch(d: LayoutDescriptor): d is LayoutBranchDescriptor {
  return 'children' in d && 'direction' in d
}

/**
 * Convert a backend direction string to MosaicDirection.
 * "horizontal" -> split side-by-side -> 'row'
 * "vertical"   -> split top/bottom  -> 'column'
 */
function toMosaicDirection(dir: 'horizontal' | 'vertical'): MosaicDirection {
  return dir === 'horizontal' ? 'row' : 'column'
}

/**
 * Recursively build a MosaicNode tree and collect PaneGroups
 * from a layout descriptor.
 */
function buildMosaicFromDescriptor(
  descriptor: LayoutDescriptor,
  paneGroups: Map<string, PaneGroup>
): MosaicNode<string> {
  if (isLayoutBranch(descriptor)) {
    const first = buildMosaicFromDescriptor(descriptor.children[0], paneGroups)
    const second = buildMosaicFromDescriptor(descriptor.children[1], paneGroups)
    return {
      direction: toMosaicDirection(descriptor.direction),
      first,
      second,
      splitPercentage: descriptor.splitPercentage ?? 50
    }
  }

  // Leaf node
  const paneGroupId = crypto.randomUUID()

  if (descriptor.type === 'document') {
    const pg = makeFileViewerPaneGroup(paneGroupId, descriptor.path ?? '', true)
    paneGroups.set(paneGroupId, pg)
  } else {
    const pg = makeTerminalPaneGroup(paneGroupId, descriptor.cwd ?? '~', {
      command: descriptor.command,
    })
    paneGroups.set(paneGroupId, pg)
  }

  return paneGroupId
}

// ── Wire up Tauri event listeners for workspace operations ───────────────

async function initWorkspaceOpsListeners(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event')
    const store = useTabsStore

    // workspace:split-pane -> split an existing pane in a tab
    listen<WsSplitPanePayload>('workspace:split-pane', (event) => {
      const { tabId, paneId, direction } = event.payload
      const newPaneGroupId = crypto.randomUUID()
      const newPane: TerminalPaneData = {
        type: 'terminal',
        terminalId: newPaneGroupId,
        cwd: '~'
      }
      store.getState().splitPane(
        tabId,
        paneId,
        newPaneGroupId,
        newPane,
        toMosaicDirection(direction)
      )
    })

    // workspace:close-pane -> remove a paneGroup from a tab
    listen<WsClosePanePayload>('workspace:close-pane', (event) => {
      const { tabId, paneId } = event.payload
      store.getState().removePaneFromTab(tabId, paneId)
    })

    // workspace:open-document -> add a file-viewer item to the paneGroup
    listen<WsOpenDocumentPayload>('workspace:open-document', (event) => {
      const { tabId, paneId, filePath } = event.payload
      const state = store.getState()
      const tab = state.tabs.find((t) => t.id === tabId)
      if (!tab) return

      if (tab.paneGroups.has(paneId)) {
        // Add a file-viewer item to the existing paneGroup
        const newItem: Item = {
          id: crypto.randomUUID(),
          type: 'file-viewer',
          data: { filePath },
          pinned: true,
        }
        state.addItemToPaneGroup(tabId, paneId, newItem)
      } else {
        // PaneGroup doesn't exist — use the store's openFileInPane
        state.openFileInPane(tabId, filePath)
      }
    })

    // workspace:open-terminal -> add a terminal item or create a new paneGroup
    listen<WsOpenTerminalPayload>('workspace:open-terminal', (event) => {
      const { tabId, paneId, cwd, command } = event.payload
      const state = store.getState()
      const tab = state.tabs.find((t) => t.id === tabId)
      if (!tab) return

      if (tab.paneGroups.has(paneId)) {
        // Add a terminal item to the existing paneGroup
        const newItem: Item = {
          id: crypto.randomUUID(),
          type: 'terminal',
          data: {
            terminalId: paneId,
            cwd,
            command,
          },
        }
        state.addItemToPaneGroup(tabId, paneId, newItem)
      } else {
        // PaneGroup doesn't exist — create it and add to mosaic
        const pg = makeTerminalPaneGroup(paneId, cwd, { command })
        state.addPaneToTab(tabId, paneId, {
          type: 'terminal',
          terminalId: paneId,
          cwd,
          command,
        })

        const existingLeaf = getFirstLeaf(tab.mosaicTree)
        if (existingLeaf && tab.mosaicTree !== null) {
          const splitNode: MosaicNode<string> = {
            direction: 'column',
            first: existingLeaf,
            second: paneId,
            splitPercentage: 50
          }
          const newTree = replaceInTree(tab.mosaicTree, existingLeaf, splitNode)
          if (newTree) {
            state.updateMosaicTree(tabId, newTree)
          }
        } else {
          state.updateMosaicTree(tabId, paneId)
        }
      }
    })

    // workspace:new-tab -> create a new tab
    listen<WsNewTabPayload>('workspace:new-tab', (event) => {
      const { cwd } = event.payload
      store.getState().addTab(cwd)
    })

    // workspace:close-tab -> close a tab
    listen<WsCloseTabPayload>('workspace:close-tab', (event) => {
      const { tabId } = event.payload
      store.getState().removeTab(tabId)
    })

    // workspace:arrange -> build a full layout from a descriptor
    listen<LayoutDescriptor>('workspace:arrange', (event) => {
      const descriptor = event.payload
      const state = store.getState()

      const paneGroups = new Map<string, PaneGroup>()
      const mosaicTree = buildMosaicFromDescriptor(descriptor, paneGroups)

      tabCounter++
      const tabId = crypto.randomUUID()

      // Derive tab title from the first paneGroup
      let title = `Layout ${tabCounter}`
      for (const pg of paneGroups.values()) {
        const firstItem = pg.items[0]
        if (firstItem?.type === 'file-viewer') {
          const d = firstItem.data as FileViewerItemData
          title = d.filePath.split('/').pop() ?? title
          break
        }
      }

      const tab: Tab = {
        id: tabId,
        title,
        mosaicTree,
        paneGroups
      }

      // Add the arranged tab and make it active
      store.setState((s) => ({
        tabs: [...s.tabs, tab],
        activeTabId: tabId
      }))
    })
  } catch {
    // Tauri API not available (e.g. in tests)
  }
}

// Initialize listeners on import
initWorkspaceOpsListeners()

// Load persisted workspace sessions from DB on import
useTabsStore.getState().loadWorkspaceSessionsFromDb()

// Auto-save: subscribe to tab structure changes and persist to DB (debounced).
// This provides crash resilience — lose at most ~1 second of tab changes.
useTabsStore.subscribe(
  (state, prevState) => {
    // Only trigger on meaningful tab structure changes (not backgroundWorkspaces swaps)
    if (
      state.tabs !== prevState.tabs ||
      state.extraGroups !== prevState.extraGroups ||
      state.splitCount !== prevState.splitCount ||
      state.activeTabId !== prevState.activeTabId ||
      state.activeGroupIndex !== prevState.activeGroupIndex
    ) {
      if (state.activeWorkspaceKey && state.tabs.length > 0) {
        state.persistActiveWorkspace()
      }
    }
  }
)

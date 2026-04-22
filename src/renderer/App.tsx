import React, { useState, useEffect, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import Layout from './components/Layout/Layout'
import FocusLayout from './components/Layout/FocusLayout'
import Sidebar from './components/Sidebar/Sidebar'
import FileTree from './components/FileTree/FileTree'
import ChangesPanel from './components/ChangesPanel/ChangesPanel'
import ChatHistory from './components/ChatHistory/ChatHistory'
import WorkspacePanel from './components/WorkspacePanel/WorkspacePanel'
import ReviewQueueModal from './components/ReviewQueueModal/ReviewQueueModal'
import { useReviewQueueStore, startReviewQueuePolling, stopReviewQueuePolling } from './stores/review-queue'
import TabbedPanel from './components/TabbedPanel/TabbedPanel'
import { TerminalArea } from './components/Terminal/TerminalArea'
import Settings from './components/Settings/Settings'
import GitInitDialog from './components/GitInitDialog/GitInitDialog'
import AddWorkspaceDialog from './components/AddWorkspaceDialog/AddWorkspaceDialog'
import RemoveWorkspaceDialog from './components/RemoveWorkspaceDialog/RemoveWorkspaceDialog'
import WorktreeBar from './components/FocusWindow/WorktreeBar'
import CommandPalette from './components/CommandPalette/CommandPalette'
import ContextMenu from './components/ContextMenu/ContextMenu'
import ConfirmDialog from './components/ConfirmDialog/ConfirmDialog'
import HeartbeatScheduleDialog from './components/HeartbeatScheduleDialog/HeartbeatScheduleDialog'
import MergeDialog from './components/MergeDialog/MergeDialog'
import Toast from './components/Toast/Toast'
import AssistantBar from './components/WorkspaceAssistant/AssistantBar'
import { useProjectsStore } from './stores/projects'
import { usePanelsStore } from './stores/panels'
import { useSettingsStore } from './stores/settings'
import { useCommandPaletteStore } from './stores/command-palette'
import { useRunningAgentsStore } from './stores/running-agents'
import RunningAgentsPanel from './components/RunningAgentsPanel/RunningAgentsPanel'
import { useTerminalSettingsStore } from './stores/terminal-settings'
import { useAssistantStore } from './stores/assistant'
import { useTabsStore } from './stores/tabs'
import { useSidebarStore } from './stores/sidebar'
import { useActiveAgentsStore, startAgentPolling, stopAgentPolling } from './stores/active-agents'
import AgentCloseDialog from './components/AgentCloseDialog/AgentCloseDialog'
import FocusWorkspaceHeader from './components/FocusWindow/FocusWorkspaceHeader'
import { useGitInfo } from './hooks/useGit'
import { useUpdateChecker } from './hooks/useUpdateChecker'
import { useWindowSync } from './hooks/useWindowSync'
import { useTimerStore } from './stores/timer'
import CountdownOverlay from './components/Timer/CountdownOverlay'
import MemoDialog from './components/Timer/MemoDialog'
import ExtendTimerDialog from './components/Timer/ExtendTimerDialog'
import { useCursorMigrationCheck } from './hooks/useCursorMigrationCheck'
import { HarnessLab } from './kessel/HarnessLab'
import { prewarmDaemonWs } from './kessel/daemon-ws'

/** Parse focus mode project ID from URL hash (#focus=<projectId>) */
function parseFocusProjectId(): string | null {
  const hash = window.location.hash
  if (!hash) return null
  const match = hash.match(/^#focus=(.+)$/)
  if (!match) return null
  return decodeURIComponent(match[1])
}

function LeftPanelContent({ rootPath, header }: { rootPath?: string; header?: React.ReactNode }): React.JSX.Element {
  const tabs = usePanelsStore((s) => s.leftPanelTabs)
  const activeTab = usePanelsStore((s) => s.leftPanelActiveTab)
  const setActiveTab = usePanelsStore((s) => s.setLeftPanelActiveTab)
  const width = usePanelsStore((s) => s.leftPanelWidth)
  const setWidth = usePanelsStore((s) => s.setLeftPanelWidth)

  if (tabs.length === 0) return <></>

  return (
    <TabbedPanel
      tabs={tabs}
      activeTab={activeTab}
      onTabChange={setActiveTab}
      width={width}
      onWidthChange={setWidth}
      resizeSide="right"
      header={header}
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {activeTab === 'history' && <ChatHistory />}
      {activeTab === 'workspace' && <WorkspacePanel />}
    </TabbedPanel>
  )
}

function RightPanelContent({ rootPath, header }: { rootPath?: string; header?: React.ReactNode }): React.JSX.Element {
  const tabs = usePanelsStore((s) => s.rightPanelTabs)
  const activeTab = usePanelsStore((s) => s.rightPanelActiveTab)
  const setActiveTab = usePanelsStore((s) => s.setRightPanelActiveTab)
  const width = usePanelsStore((s) => s.rightPanelWidth)
  const setWidth = usePanelsStore((s) => s.setRightPanelWidth)

  if (tabs.length === 0) return <></>

  return (
    <TabbedPanel
      tabs={tabs}
      activeTab={activeTab}
      onTabChange={setActiveTab}
      width={width}
      onWidthChange={setWidth}
      resizeSide="left"
      header={header}
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {activeTab === 'history' && <ChatHistory />}
      {activeTab === 'workspace' && <WorkspacePanel />}
    </TabbedPanel>
  )
}

class FocusErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }
  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }
  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[FocusModeContent] CRASH:', error, info.componentStack)
  }
  render(): React.ReactNode {
    if (this.state.error) {
      return (
        <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-red-400 text-xs p-8">
          <div>
            <p className="font-bold mb-2">Focus window error:</p>
            <pre className="whitespace-pre-wrap">{this.state.error.message}</pre>
            <pre className="whitespace-pre-wrap text-[var(--color-text-muted)] mt-2">{this.state.error.stack}</pre>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}

function FocusModeContent({ activeProject, cwd }: { activeProject: any; cwd: string }): React.JSX.Element {
  const focusHeaderSide = usePanelsStore((s) => s.focusWorkspaceHeaderSide)
  const leftHeader = focusHeaderSide === 'left' ? <FocusWorkspaceHeader side="left" /> : undefined
  const rightHeader = focusHeaderSide === 'right' ? <FocusWorkspaceHeader side="right" /> : undefined
  const { data: gitInfo } = useGitInfo(activeProject?.path)

  return (
    <FocusErrorBoundary>
      <FocusLayout
        projectName={activeProject?.name}
        branchName={gitInfo?.isRepo ? gitInfo.currentBranch : undefined}
        leftPanel={<LeftPanelContent rootPath={cwd} header={leftHeader} />}
        rightPanel={<RightPanelContent rootPath={cwd} header={rightHeader} />}
      >
        {activeProject ? (
          <TerminalArea cwd={cwd} />
        ) : (
          <div className="flex-1 flex items-center justify-center h-full">
            <div className="text-center">
              <h2 className="text-lg font-medium text-[var(--color-text-muted)]">Loading...</h2>
            </div>
          </div>
        )}
      </FocusLayout>
      <GitInitDialog />
      <AddWorkspaceDialog />
      <RemoveWorkspaceDialog />
      <CommandPalette />
      <ReviewQueueModal />
      <RunningAgentsPanel />
      <ContextMenu />
      <ConfirmDialog />
      <HeartbeatScheduleDialog />
      <MergeDialog />
      <Toast />
      <AssistantBar />
      <CountdownOverlay />
      <MemoDialog />
      <ExtendTimerDialog />
    </FocusErrorBoundary>
  )
}

// ── App Zoom ─────────────────────────────────────────────────────────────
// Uses CSS `zoom` on #root for crisp text at any zoom level.
// Native WKWebView zoom is disabled via zoomHotkeysEnabled:false.
declare global {
  interface Window { __k2soZoom?: number }
}

function applyK2SOZoom(): void {
  const z = window.__k2soZoom ?? 1
  if (z === 1) {
    document.documentElement.style.zoom = ''
    document.title = 'K2SO'
  } else {
    document.documentElement.style.zoom = String(z)
    document.title = `K2SO — ${Math.round(z * 100)}%`
  }
}

export default function App(): React.JSX.Element {
  const settingsLoaded = useSettingsStore((s) => s.loaded)
  const focusProjectId = useMemo(() => parseFocusProjectId(), [])
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const projects = useProjectsStore((s) => s.projects)
  const [focusInitialized, setFocusInitialized] = useState(false)

  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const openSettings = useSettingsStore((s) => s.openSettings)

  // Phase 4.5 — Kessel Harness Lab (Cmd+Shift+K). Dev/visual
  // validation surface for the new Session Stream pipeline. Not
  // persisted to any store since it's intended as a sandbox.
  const [kesselLabOpen, setKesselLabOpen] = useState(false)

  const toggleCommandPalette = useCommandPaletteStore((s) => s.toggle)

  const toggleAssistant = useAssistantStore((s) => s.toggle)
  const toggleReviewQueue = useReviewQueueStore((s) => s.toggle)
  const toggleRunningAgents = useRunningAgentsStore((s) => s.toggle)

  // Prewarm the daemon_ws_url cache at app mount. The underlying
  // Tauri command reads two files from ~/.k2so/ on every call; the
  // result is stable for the app session so one fetch is enough.
  // Fire-and-forget — the first Kessel pane would trigger this
  // anyway, but kicking it off at mount hides the ~5-10ms disk I/O
  // behind the rest of the initial render.
  useEffect(() => {
    prewarmDaemonWs()
  }, [])

  // Cmd+, settings, Cmd+K command palette, Cmd+L assistant, Cmd+P review queue
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.metaKey && e.key === ',') {
        e.preventDefault()
        openSettings()
      }
      if (e.metaKey && e.key === 'k') {
        e.preventDefault()
        toggleCommandPalette()
      }
      if (e.metaKey && e.key === 'l') {
        e.preventDefault()
        toggleAssistant()
      }
      if (e.metaKey && e.key === 'p') {
        e.preventDefault()
        toggleReviewQueue()
      }
      if (e.metaKey && e.key === 'j') {
        e.preventDefault()
        toggleRunningAgents()
      }
      // Cmd+Shift+K — open the Kessel Harness Lab (Phase 4.5).
      if (e.metaKey && e.shiftKey && (e.key === 'K' || e.key === 'k')) {
        e.preventDefault()
        setKesselLabOpen((v) => !v)
      }
      // Cmd+[ to go back, Cmd+] to go forward
      if (e.metaKey && !e.shiftKey && e.key === '[') {
        e.preventDefault()
        useTabsStore.getState().goBack()
      }
      if (e.metaKey && !e.shiftKey && e.key === ']') {
        e.preventDefault()
        useTabsStore.getState().goForward()
      }
      // Cmd+Shift++ to increase terminal font size
      if (e.metaKey && e.shiftKey && e.key === '+') {
        e.preventDefault()
        useTerminalSettingsStore.getState().incrementFontSize()
      }
      // Cmd+Shift+- to decrease terminal font size
      if (e.metaKey && e.shiftKey && e.key === '-') {
        e.preventDefault()
        useTerminalSettingsStore.getState().decrementFontSize()
      }
      // App zoom — scales #root via transform and adjusts its dimensions to fill the window
      if (e.metaKey && !e.shiftKey && (e.key === '=' || e.key === '+')) {
        e.preventDefault()
        window.__k2soZoom = Math.min(Math.round(((window.__k2soZoom ?? 1) + 0.1) * 10) / 10, 2.0)
        applyK2SOZoom()
      }
      if (e.metaKey && !e.shiftKey && e.key === '-') {
        e.preventDefault()
        window.__k2soZoom = Math.max(Math.round(((window.__k2soZoom ?? 1) - 0.1) * 10) / 10, 0.5)
        applyK2SOZoom()
      }
      if (e.metaKey && !e.shiftKey && e.key === '0') {
        e.preventDefault()
        window.__k2soZoom = 1
        applyK2SOZoom()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [openSettings, toggleCommandPalette, toggleAssistant, toggleReviewQueue, toggleRunningAgents])

  // Refocus last active terminal when clicking dead space (navbar, sidebar padding, etc.)
  // Interactive elements (inputs, textareas, buttons, contenteditable, select) keep their own focus.
  useEffect(() => {
    const handleGlobalClick = (e: MouseEvent) => {
      const target = e.target as HTMLElement
      if (!target) return

      // Don't steal focus from interactive elements
      const tag = target.tagName.toLowerCase()
      if (tag === 'input' || tag === 'textarea' || tag === 'select' || tag === 'button') return
      if (target.isContentEditable) return
      if (target.closest('input, textarea, select, button, [contenteditable="true"], [role="textbox"]')) return
      // Don't steal from elements with tabindex (custom interactive components)
      if (target.tabIndex >= 0 && target.dataset.terminalContainer === undefined) return

      // Find the last focused terminal container and refocus it
      requestAnimationFrame(() => {
        const activeEl = document.activeElement
        // If something interactive already grabbed focus, leave it
        if (activeEl && (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA' || (activeEl as HTMLElement).isContentEditable)) return

        // Find the visible terminal container in the active tab
        const terminalContainer = document.querySelector('[data-terminal-container][data-terminal-visible="true"]') as HTMLElement
        if (terminalContainer) {
          terminalContainer.focus()
        }
      })
    }

    // Refocus terminal when nothing has focus (after modal close, Esc, etc.)
    // Polls every 200ms — if activeElement is body or null (nothing focused)
    // and no overlay is open, refocus the terminal.
    const refocusInterval = setInterval(() => {
      const active = document.activeElement
      // Only refocus if nothing meaningful has focus
      if (active && active !== document.body) return
      // Don't refocus if settings is open
      if (useSettingsStore.getState().settingsOpen) return
      // Don't refocus if any overlay is open (command palette, running agents, assistant)
      if (useCommandPaletteStore.getState().isOpen) return
      if (useRunningAgentsStore.getState().isOpen) return
      // Find and focus the visible terminal
      const terminalContainer = document.querySelector('[data-terminal-container][data-terminal-visible="true"]') as HTMLElement
      if (terminalContainer) {
        terminalContainer.focus()
      }
    }, 200)

    document.addEventListener('click', handleGlobalClick, true) // capture phase
    return () => {
      document.removeEventListener('click', handleGlobalClick, true)
      clearInterval(refocusInterval)
    }
  }, [])

  // Listen for menu events from Tauri backend
  useEffect(() => {
    const unlisteners: Array<() => void> = []
    import('@tauri-apps/api/event').then(({ listen }) => {
      listen('menu:open-settings', () => openSettings()).then((fn) => unlisteners.push(fn))
      listen('menu:check-for-updates', () => {
        useSettingsStore.setState({ pendingUpdateCheck: true })
        openSettings('general')
      }).then((fn) => unlisteners.push(fn))
      listen('menu:new-document', () => {
        const ps = useProjectsStore.getState()
        const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
        const ws = proj?.workspaces.find((w) => w.id === ps.activeWorkspaceId)
        const cwd = ws?.worktreePath ?? proj?.path ?? '~'
        useTabsStore.getState().openUntitledDocument(cwd)
      }).then((fn) => unlisteners.push(fn))
      listen('menu:new-tab', () => {
        const ps = useProjectsStore.getState()
        const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
        const ws = proj?.workspaces.find((w) => w.id === ps.activeWorkspaceId)
        const cwd = ws?.worktreePath ?? proj?.path ?? '~'
        useTabsStore.getState().addTab(cwd)
      }).then((fn) => unlisteners.push(fn))
      listen('menu:launch-agent', async () => {
        const ps = useProjectsStore.getState()
        const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
        const ws = proj?.workspaces.find((w) => w.id === ps.activeWorkspaceId)
        const cwd = ws?.worktreePath ?? proj?.path ?? '~'
        const { usePresetsStore: presetsStore } = await import('@/stores/presets')
        const state = presetsStore.getState()
        const defaultPreset = state.presets.find((p: any) => p.enabled)
        if (defaultPreset) {
          state.launchPreset(defaultPreset.id, cwd, 'tab')
        }
      }).then((fn) => unlisteners.push(fn))
      listen('menu:split-pane', () => {
        const tabsState = useTabsStore.getState()
        const activeTab = tabsState.tabs.find((t) => t.id === tabsState.activeTabId)
        if (!activeTab) return
        const getLeaf = (t: unknown): string | null => {
          if (!t) return null
          if (typeof t === 'string') return t
          if (typeof t === 'object' && t !== null && 'first' in t) return getLeaf((t as any).first)
          return null
        }
        const firstPaneId = getLeaf(activeTab.mosaicTree)
        if (!firstPaneId) return
        const ps = useProjectsStore.getState()
        const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
        const ws = proj?.workspaces.find((w) => w.id === ps.activeWorkspaceId)
        const cwd = ws?.worktreePath ?? proj?.path ?? '~'
        const newPaneId = crypto.randomUUID()
        tabsState.splitPane(activeTab.id, firstPaneId, newPaneId, { type: 'terminal', terminalId: newPaneId, cwd }, 'column')
      }).then((fn) => unlisteners.push(fn))
      listen('menu:open-workspace', () => {
        import('@tauri-apps/api/core').then(({ invoke }) => {
          invoke<string | null>('projects_pick_folder').then((path) => {
            if (path) useProjectsStore.getState().addProject(path)
          })
        })
      }).then((fn) => unlisteners.push(fn))
      listen('menu:close-tab', () => {
        const { activeTabId, removeTab } = useTabsStore.getState()
        if (activeTabId) removeTab(activeTabId)
      }).then((fn) => unlisteners.push(fn))
      listen('menu:command-palette', () => {
        toggleCommandPalette()
      }).then((fn) => unlisteners.push(fn))
      listen('menu:review-queue', () => {
        useReviewQueueStore.getState().toggle()
      }).then((fn) => unlisteners.push(fn))
      listen('menu:toggle-sidebar', () => {
        useSidebarStore.getState().toggle()
      }).then((fn) => unlisteners.push(fn))
      listen('menu:toggle-assistant', () => {
        toggleAssistant()
      }).then((fn) => unlisteners.push(fn))
      listen('menu:focus-window', () => {
        const projectId = useProjectsStore.getState().activeProjectId
        if (projectId) {
          import('@tauri-apps/api/core').then(({ invoke }) => {
            invoke('projects_open_focus_window', { projectId }).catch((e) => console.warn('[app]', e))
          })
        }
      }).then((fn) => unlisteners.push(fn))
      // Zoom events from menu — use native WKWebView zoom via Tauri API
      listen('app:zoom-in', () => {
        import('@tauri-apps/api/webview').then(m => m.getCurrentWebview().setZoom(
          (window as any).__k2soNativeZoom = Math.min(((window as any).__k2soNativeZoom ?? 1) + 0.2, 3)
        ))
      }).then((fn) => unlisteners.push(fn))
      listen('app:zoom-out', () => {
        import('@tauri-apps/api/webview').then(m => m.getCurrentWebview().setZoom(
          (window as any).__k2soNativeZoom = Math.max(((window as any).__k2soNativeZoom ?? 1) - 0.2, 0.4)
        ))
      }).then((fn) => unlisteners.push(fn))
      listen('app:zoom-reset', () => {
        (window as any).__k2soNativeZoom = 1
        import('@tauri-apps/api/webview').then(m => m.getCurrentWebview().setZoom(1))
      }).then((fn) => unlisteners.push(fn))
    })
    return () => {
      unlisteners.forEach((fn) => fn())
    }
  }, [openSettings, toggleAssistant, toggleCommandPalette])

  // In focus mode, set the active project to the focused project on mount
  useEffect(() => {
    if (focusProjectId && projects.length > 0 && !focusInitialized) {
      const focusProject = projects.find((p) => p.id === focusProjectId)
      if (focusProject) {
        setActiveProject(focusProject.id)
        setFocusInitialized(true)
      }
    }
  }, [focusProjectId, projects, focusInitialized, setActiveProject])

  const [showQuitDialog, setShowQuitDialog] = useState(false)
  const [quitAgents, setQuitAgents] = useState<ReturnType<typeof useActiveAgentsStore.getState>['getActiveAgentsList']>([])

  // Start agent polling + review queue polling (only when agentic systems enabled)
  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  useEffect(() => {
    if (agenticEnabled) {
      startAgentPolling()
      startReviewQueuePolling()
    }
    return () => { stopAgentPolling(); stopReviewQueuePolling() }
  }, [agenticEnabled])

  // Check for updates on launch and every 3 hours
  useUpdateChecker()

  // Sync state across all windows (main, focus, new)
  useWindowSync()

  // macOS close button dot — show when active agents or dirty tabs
  const agentCount = useActiveAgentsStore((s) => s.agents.size)
  const workingPanes = useActiveAgentsStore((s) => {
    let count = 0
    for (const status of s.paneStatuses.values()) {
      if (status === 'working' || status === 'permission') count++
    }
    return count
  })
  const hasDirtyTabs = useTabsStore((s) => s.tabs.some((t) => t.isDirty))
  useEffect(() => {
    const edited = agentCount > 0 || workingPanes > 0 || hasDirtyTabs
    invoke('set_document_edited', { edited }).catch(() => {})
  }, [agentCount, workingPanes, hasDirtyTabs])

  // Check for unmigrated Cursor IDE conversations
  useCursorMigrationCheck()

  // Save all workspaces (active + background) to DB
  const saveAllWorkspaces = async (): Promise<void> => {
    const tabsStore = useTabsStore.getState()
    const projectsStore = useProjectsStore.getState()
    const activeKey = projectsStore.activeProjectId && projectsStore.activeWorkspaceId
      ? `${projectsStore.activeProjectId}:${projectsStore.activeWorkspaceId}`
      : null
    if (activeKey) {
      await tabsStore.serializeAllWorkspaces(activeKey)
    }
  }

  // Force quit (bypasses agent check)
  const forceQuit = async (): Promise<void> => {
    setShowQuitDialog(false)
    // Auto-stop timer silently
    const timerState = useTimerStore.getState()
    if (timerState.status !== 'idle') {
      await timerState.stopTimerSilently()
    }
    const tabsStore = useTabsStore.getState()
    await tabsStore.detectAndSaveSessionIds()
    await saveAllWorkspaces()
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    getCurrentWindow().destroy()
  }

  // Before app close: check for active agents, save layout
  useEffect(() => {
    const handleBeforeUnload = (): void => {
      // Auto-stop timer silently on close (no memo prompt)
      const timerState = useTimerStore.getState()
      if (timerState.status !== 'idle') {
        timerState.stopTimerSilently()
      }
      // Best-effort sync save (beforeunload can't await)
      const tabsStore = useTabsStore.getState()
      const projectsStore = useProjectsStore.getState()
      const activeKey = projectsStore.activeProjectId && projectsStore.activeWorkspaceId
        ? `${projectsStore.activeProjectId}:${projectsStore.activeWorkspaceId}`
        : null
      if (activeKey) {
        tabsStore.saveLayoutForWorkspace(projectsStore.activeProjectId!, projectsStore.activeWorkspaceId!)
      }
    }

    window.addEventListener('beforeunload', handleBeforeUnload)

    let unlisten: (() => void) | undefined
    import('@tauri-apps/api/event').then(({ listen }) => {
      listen('tauri://close-requested', async (event) => {
        // Always prevent default so we can save before closing
        ;(event as any).preventDefault?.()

        // Auto-stop timer silently before closing
        const timerState = useTimerStore.getState()
        if (timerState.status !== 'idle') {
          await timerState.stopTimerSilently()
        }

        // Check for active agents
        await useActiveAgentsStore.getState().pollOnce()
        const agents = useActiveAgentsStore.getState().getActiveAgentsList()

        if (agents.length > 0) {
          setQuitAgents(agents)
          setShowQuitDialog(true)
          return
        }

        // No agents — save all workspaces (active + background) with session IDs, then close
        const tabsStore = useTabsStore.getState()
        await tabsStore.detectAndSaveSessionIds()
        await saveAllWorkspaces()
        const { getCurrentWindow } = await import('@tauri-apps/api/window')
        getCurrentWindow().destroy()
      }).then((fn) => { unlisten = fn }).catch((e) => console.warn('[app]', e))
    })

    return () => {
      window.removeEventListener('beforeunload', handleBeforeUnload)
      unlisten?.()
    }
  }, [])

  const effectiveProjectId = focusProjectId ?? activeProjectId
  const activeProject = projects.find((p) => p.id === effectiveProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const cwd = activeWorkspace?.worktreePath ?? activeProject?.path ?? '~'

  // Wait for stores to initialize before rendering to prevent flicker
  if (!settingsLoaded) {
    return <div className="h-full w-full bg-[var(--color-bg)]" />
  }

  // Settings overlay — replaces main content
  if (settingsOpen) {
    return (
      <>
        <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg)]">
          <div
            className="h-[38px] flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)]"
            data-tauri-drag-region
            onMouseDown={() => {
              import('@tauri-apps/api/window').then(m => m.getCurrentWindow().startDragging())
            }}
          />
          <div className="flex-1 min-h-0">
            <Settings />
          </div>
        </div>
        <GitInitDialog />
      <AddWorkspaceDialog />
      <RemoveWorkspaceDialog />
        <CommandPalette />
        <ContextMenu />
        <ConfirmDialog />
        <HeartbeatScheduleDialog />
      <MergeDialog />
        <Toast />
        <AssistantBar />
        <CountdownOverlay />
        <MemoDialog />
      <ExtendTimerDialog />
      <HarnessLab open={kesselLabOpen} onClose={() => setKesselLabOpen(false)} />
      </>
    )
  }

  // Focus mode: workspace header above sidebar tabs, no primary sidebar
  if (focusProjectId) {
    return (
      <FocusModeContent activeProject={activeProject} cwd={cwd} />
    )
  }

  return (
    <>
      <Layout
        sidebar={<Sidebar />}
        leftPanel={<LeftPanelContent rootPath={activeWorkspace?.worktreePath ?? activeProject?.path} />}
        rightPanel={<RightPanelContent rootPath={activeWorkspace?.worktreePath ?? activeProject?.path} />}
        projectName={activeProject?.name}
        workspaceName={activeWorkspace?.name}
      >
        {activeProject && activeWorkspace ? (
          <TerminalArea cwd={cwd} />
        ) : (
          <div className="flex-1 flex items-center justify-center h-full">
            <div className="text-center">
              <h2 className="text-lg font-medium text-[var(--color-text-muted)]">K2SO</h2>
              <p className="text-xs text-[var(--color-text-muted)] mt-2 opacity-60">
                Add a workspace to get started
              </p>
            </div>
          </div>
        )}
      </Layout>
      <GitInitDialog />
      <AddWorkspaceDialog />
      <RemoveWorkspaceDialog />
      <CommandPalette />
      <ReviewQueueModal />
      <RunningAgentsPanel />
      <ContextMenu />
      <ConfirmDialog />
      <HeartbeatScheduleDialog />
      <MergeDialog />
      <Toast />
      <AssistantBar />
      <CountdownOverlay />
      <MemoDialog />
      <ExtendTimerDialog />
      <HarnessLab open={kesselLabOpen} onClose={() => setKesselLabOpen(false)} />
      {showQuitDialog && (
        <AgentCloseDialog
          agents={quitAgents}
          mode="app"
          onConfirm={forceQuit}
          onCancel={() => setShowQuitDialog(false)}
        />
      )}
    </>
  )
}

import React, { useState, useEffect, useMemo } from 'react'
import Layout from './components/Layout/Layout'
import FocusLayout from './components/Layout/FocusLayout'
import Sidebar from './components/Sidebar/Sidebar'
import FileTree from './components/FileTree/FileTree'
import ChangesPanel from './components/ChangesPanel/ChangesPanel'
import ChatHistory from './components/ChatHistory/ChatHistory'
import TabbedPanel from './components/TabbedPanel/TabbedPanel'
import { TerminalArea } from './components/Terminal/TerminalArea'
import Settings from './components/Settings/Settings'
import GitInitDialog from './components/GitInitDialog/GitInitDialog'
import WorktreeBar from './components/FocusWindow/WorktreeBar'
import CommandPalette from './components/CommandPalette/CommandPalette'
import ContextMenu from './components/ContextMenu/ContextMenu'
import ConfirmDialog from './components/ConfirmDialog/ConfirmDialog'
import MergeDialog from './components/MergeDialog/MergeDialog'
import Toast from './components/Toast/Toast'
import AssistantBar from './components/WorkspaceAssistant/AssistantBar'
import { useProjectsStore } from './stores/projects'
import { usePanelsStore } from './stores/panels'
import { useSettingsStore } from './stores/settings'
import { useCommandPaletteStore } from './stores/command-palette'
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
        <div className="flex h-screen w-screen items-center justify-center bg-[var(--color-bg)] text-red-400 text-xs p-8">
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
        leftPanel={<LeftPanelContent rootPath={activeProject?.path} header={leftHeader} />}
        rightPanel={<RightPanelContent rootPath={activeProject?.path} header={rightHeader} />}
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
      <CommandPalette />
      <ContextMenu />
      <ConfirmDialog />
      <MergeDialog />
      <Toast />
      <AssistantBar />
      <CountdownOverlay />
      <MemoDialog />
      <ExtendTimerDialog />
    </FocusErrorBoundary>
  )
}

export default function App(): React.JSX.Element {
  const focusProjectId = useMemo(() => parseFocusProjectId(), [])
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const projects = useProjectsStore((s) => s.projects)
  const [focusInitialized, setFocusInitialized] = useState(false)

  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const openSettings = useSettingsStore((s) => s.openSettings)

  const toggleCommandPalette = useCommandPaletteStore((s) => s.toggle)

  const toggleAssistant = useAssistantStore((s) => s.toggle)

  // Cmd+, to open settings, Cmd+K to toggle command palette, Cmd+L to toggle assistant
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
      // Cmd+= (plus) to zoom in the entire app
      if (e.metaKey && !e.shiftKey && (e.key === '=' || e.key === '+')) {
        e.preventDefault()
        const current = parseFloat(document.documentElement.style.zoom || '1')
        const next = Math.min(current + 0.1, 2.0)
        document.documentElement.style.zoom = String(next)
      }
      // Cmd+- (minus) to zoom out the entire app
      if (e.metaKey && !e.shiftKey && e.key === '-') {
        e.preventDefault()
        const current = parseFloat(document.documentElement.style.zoom || '1')
        const next = Math.max(current - 0.1, 0.5)
        document.documentElement.style.zoom = String(next)
      }
      // Cmd+0 to reset app zoom
      if (e.metaKey && !e.shiftKey && e.key === '0') {
        e.preventDefault()
        document.documentElement.style.zoom = '1'
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [openSettings, toggleCommandPalette, toggleAssistant])

  // Listen for menu events from Tauri backend
  useEffect(() => {
    const unlisteners: Array<() => void> = []
    import('@tauri-apps/api/event').then(({ listen }) => {
      listen('menu:open-settings', () => openSettings()).then((fn) => unlisteners.push(fn))
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
            invoke('projects_open_focus_window', { projectId }).catch(() => {})
          })
        }
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

  // Start agent polling
  useEffect(() => {
    startAgentPolling()
    return () => stopAgentPolling()
  }, [])

  // Check for updates on launch and every 3 hours
  useUpdateChecker()

  // Sync state across all windows (main, focus, new)
  useWindowSync()

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
      }).then((fn) => { unlisten = fn }).catch(() => {})
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

  // Settings overlay — replaces main content
  if (settingsOpen) {
    return (
      <>
        <div className="flex h-screen w-screen flex-col overflow-hidden bg-[var(--color-bg)]">
          <div
            className="h-[38px] flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)]"
            data-tauri-drag-region
            onMouseDown={() => {
              import('@tauri-apps/api/window').then(m => m.getCurrentWindow().startDragging())
            }}
          />
          <div className="flex-1 overflow-hidden">
            <Settings />
          </div>
        </div>
        <GitInitDialog />
        <CommandPalette />
        <ContextMenu />
        <ConfirmDialog />
      <MergeDialog />
        <Toast />
        <AssistantBar />
        <CountdownOverlay />
        <MemoDialog />
      <ExtendTimerDialog />
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
        leftPanel={<LeftPanelContent rootPath={activeProject?.path} />}
        rightPanel={<RightPanelContent rootPath={activeProject?.path} />}
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
      <CommandPalette />
      <ContextMenu />
      <ConfirmDialog />
      <MergeDialog />
      <Toast />
      <AssistantBar />
      <CountdownOverlay />
      <MemoDialog />
      <ExtendTimerDialog />
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

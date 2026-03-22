import { useState, useEffect, useMemo } from 'react'
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
import Toast from './components/Toast/Toast'
import AssistantBar from './components/WorkspaceAssistant/AssistantBar'
import { useProjectsStore } from './stores/projects'
import { usePanelsStore } from './stores/panels'
import { useSettingsStore } from './stores/settings'
import { useCommandPaletteStore } from './stores/command-palette'
import { useTerminalSettingsStore } from './stores/terminal-settings'
import { useAssistantStore } from './stores/assistant'
import { useTabsStore } from './stores/tabs'
import { useActiveAgentsStore, startAgentPolling, stopAgentPolling } from './stores/active-agents'
import AgentCloseDialog from './components/AgentCloseDialog/AgentCloseDialog'
import { useUpdateChecker } from './hooks/useUpdateChecker'

/** Parse focus mode project ID from URL hash (#focus=<projectId>) */
function parseFocusProjectId(): string | null {
  const hash = window.location.hash
  if (!hash) return null
  const match = hash.match(/^#focus=(.+)$/)
  if (!match) return null
  return decodeURIComponent(match[1])
}

function LeftPanelContent({ rootPath }: { rootPath?: string }): React.JSX.Element {
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
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {activeTab === 'history' && <ChatHistory />}
    </TabbedPanel>
  )
}

function RightPanelContent({ rootPath }: { rootPath?: string }): React.JSX.Element {
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
    >
      {activeTab === 'files' && rootPath && <FileTree rootPath={rootPath} />}
      {activeTab === 'changes' && <ChangesPanel />}
      {activeTab === 'history' && <ChatHistory />}
    </TabbedPanel>
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

  // Listen for menu:open-settings from Tauri backend
  useEffect(() => {
    let unlisten: (() => void) | undefined
    import('@tauri-apps/api/event').then(({ listen }) => {
      listen('menu:open-settings', () => openSettings()).then((fn) => {
        unlisten = fn
      })
    })
    return () => {
      unlisten?.()
    }
  }, [openSettings])

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

  // Save layout helper
  const saveCurrentLayout = (): void => {
    const tabsStore = useTabsStore.getState()
    const projectsStore = useProjectsStore.getState()
    if (projectsStore.activeProjectId && projectsStore.activeWorkspaceId) {
      tabsStore.saveLayoutForWorkspace(
        projectsStore.activeProjectId,
        projectsStore.activeWorkspaceId
      )
    }
  }

  // Force quit (bypasses agent check)
  const forceQuit = async (): Promise<void> => {
    setShowQuitDialog(false)
    const tabsStore = useTabsStore.getState()
    await tabsStore.detectAndSaveSessionIds()
    saveCurrentLayout()
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    getCurrentWindow().destroy()
  }

  // Before app close: check for active agents, save layout
  useEffect(() => {
    const handleBeforeUnload = (): void => {
      saveCurrentLayout()
    }

    window.addEventListener('beforeunload', handleBeforeUnload)

    let unlisten: (() => void) | undefined
    import('@tauri-apps/api/event').then(({ listen }) => {
      listen('tauri://close-requested', async (event) => {
        // Check for active agents
        await useActiveAgentsStore.getState().pollOnce()
        const agents = useActiveAgentsStore.getState().getActiveAgentsList()

        if (agents.length > 0) {
          // Prevent close and show dialog
          (event as any).preventDefault?.()
          setQuitAgents(agents)
          setShowQuitDialog(true)
          return
        }

        // No agents — proceed with close
        const tabsStore = useTabsStore.getState()
        await tabsStore.detectAndSaveSessionIds()
        saveCurrentLayout()
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
        <Toast />
        <AssistantBar />
      </>
    )
  }

  // Focus mode: no workspaces sidebar, but still has left/right panels
  if (focusProjectId) {
    return (
      <>
        <FocusLayout
          projectName={activeProject?.name}
          workspaceBar={activeProject ? <WorktreeBar project={activeProject} /> : undefined}
          leftPanel={<LeftPanelContent rootPath={activeProject?.path} />}
          rightPanel={<RightPanelContent rootPath={activeProject?.path} />}
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
        <Toast />
        <AssistantBar />
      </>
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
      <Toast />
      <AssistantBar />
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

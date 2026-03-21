import { useState, useEffect, useMemo } from 'react'
import Layout from './components/Layout/Layout'
import FocusLayout from './components/Layout/FocusLayout'
import Sidebar from './components/Sidebar/Sidebar'
import FileTree from './components/FileTree/FileTree'
import ChangesPanel from './components/ChangesPanel/ChangesPanel'
import TabbedPanel from './components/TabbedPanel/TabbedPanel'
import { TerminalArea } from './components/Terminal/TerminalArea'
import Settings from './components/Settings/Settings'
import GitInitDialog from './components/GitInitDialog/GitInitDialog'
import WorktreeBar from './components/FocusWindow/WorktreeBar'
import CommandPalette from './components/CommandPalette/CommandPalette'
import ContextMenu from './components/ContextMenu/ContextMenu'
import { useProjectsStore } from './stores/projects'
import { usePanelsStore } from './stores/panels'
import { useSettingsStore } from './stores/settings'
import { useCommandPaletteStore } from './stores/command-palette'

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

  // Cmd+, to open settings, Cmd+K to toggle command palette
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
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [openSettings, toggleCommandPalette])

  // Listen for menu:open-settings from main process
  useEffect(() => {
    const handler = (): void => openSettings()
    window.api.on('menu:open-settings', handler)
    return () => window.api.off('menu:open-settings', handler)
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

  const effectiveProjectId = focusProjectId ?? activeProjectId
  const activeProject = projects.find((p) => p.id === effectiveProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const cwd = activeWorkspace?.worktreePath ?? activeProject?.path ?? '~'

  // Settings overlay — replaces main content
  if (settingsOpen) {
    return (
      <>
        <div className="flex h-screen w-screen flex-col overflow-hidden bg-[var(--color-bg)]">
          <div className="h-[38px] flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] drag" />
          <div className="flex-1 overflow-hidden">
            <Settings />
          </div>
        </div>
        <CommandPalette />
        <ContextMenu />
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
    </>
  )
}

import { useCallback, useMemo } from 'react'
import { useProjectsStore, type ProjectWithWorkspaces } from '../../stores/projects'
import { useFocusGroupsStore } from '../../stores/focus-groups'
import { useSidebarStore } from '../../stores/sidebar'
import { useSettingsStore } from '../../stores/settings'
import { useActiveAgentsStore } from '../../stores/active-agents'
import { useGitInfo, useGitChanges } from '../../hooks/useGit'
import { invoke } from '@tauri-apps/api/core'
import { showContextMenu } from '../../lib/context-menu'
import ProjectAvatar from './ProjectAvatar'

const RAIL_WIDTH = 48

function ProjectIcon({
  project,
  isActive,
  onClick,
  onContextMenu
}: {
  project: ProjectWithWorkspaces
  isActive: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
}): React.JSX.Element {
  const { data: gitInfo } = useGitInfo(project.path)
  const { data: changes } = useGitChanges(project.path)
  const agentStatus = useActiveAgentsStore((s) => s.getProjectStatus(project.id))

  const hasDirtyFiles =
    gitInfo?.isRepo && (gitInfo.changedFiles + gitInfo.untrackedFiles) > 0

  const added = changes.filter(
    (f) => f.status === 'added' || f.status === 'untracked'
  ).length
  const deleted = changes.filter((f) => f.status === 'deleted').length

  // Build tooltip: "WorkspaceName • branch • +N/-N changes"
  const tooltipParts = [project.name]
  if (gitInfo?.isRepo && gitInfo.currentBranch) {
    tooltipParts.push(gitInfo.currentBranch)
  }
  if (added > 0 || deleted > 0) {
    const diffParts: string[] = []
    if (added > 0) diffParts.push(`+${added}`)
    if (deleted > 0) diffParts.push(`-${deleted}`)
    tooltipParts.push(`${diffParts.join('/')} changes`)
  }
  const tooltip = tooltipParts.join(' \u2022 ')

  return (
    <button
      className={`no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 transition-colors ${
        isActive
          ? 'bg-white/[0.12] text-[var(--color-text-primary)] icon-rail-active'
          : 'text-[var(--color-text-muted)] hover:bg-white/[0.06] hover:text-[var(--color-text-secondary)]'
      }`}
      style={
        isActive
          ? ({ '--accent-color': project.color } as React.CSSProperties)
          : undefined
      }
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={tooltip}
    >
      <ProjectAvatar
        projectPath={project.path}
        projectName={project.name}
        projectColor={project.color}
        projectId={project.id}
        iconUrl={project.iconUrl}
        size={20}
      />
      {agentStatus === 'working' && <span className="icon-rail-badge agent-dot-working" />}
      {agentStatus === 'permission' && <span className="icon-rail-badge agent-dot-permission" />}
      {agentStatus === 'review' && <span className="icon-rail-badge agent-dot-review" />}
      {agentStatus === 'idle' && hasDirtyFiles && <span className="icon-rail-badge status-dot-dirty" />}
    </button>
  )
}

export default function IconRail(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const setActiveProject = useProjectsStore((s) => s.setActiveProject)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const addProject = useProjectsStore((s) => s.addProject)

  const expand = useSidebarStore((s) => s.expand)


  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)

  const pinnedProjects = useMemo(() =>
    projects.filter((p) => p.pinned), [projects])

  const filteredProjects = useMemo(() => {
    const unpinned = projects.filter((p) => !p.pinned)
    if (!focusGroupsEnabled || activeFocusGroupId === null) return unpinned
    return unpinned.filter((p) => p.focusGroupId === activeFocusGroupId)
  }, [projects, focusGroupsEnabled, activeFocusGroupId])

  // Combined list: pinned first, then filtered — matches expanded sidebar order
  const orderedProjects = useMemo(() =>
    [...pinnedProjects, ...filteredProjects], [pinnedProjects, filteredProjects])

  const handleAddProject = useCallback(async () => {
    const folderPath = await invoke<string | null>('projects_pick_folder')
    if (folderPath) {
      await addProject(folderPath)
    }
  }, [addProject])

  const handleProjectClick = useCallback(
    (projectId: string) => {
      setActiveProject(projectId)
    },
    [setActiveProject]
  )

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, projectId: string) => {
      e.preventDefault()
      const project = projects.find((p) => p.id === projectId)
      if (!project) return

      // Fetch installed editors
      let editors: Array<{ id: string; label: string }> = []
      try {
        editors = await invoke<Array<{ id: string; label: string }>>('projects_get_editors')
      } catch {
        // ignore
      }

      const menuItems: Array<{ id: string; label: string; type?: string }> = [
        { id: 'settings', label: 'Workspace Settings' },
        { id: 'separator-settings', label: '', type: 'separator' },
        { id: 'expand', label: 'Expand Sidebar' },
        { id: 'separator', label: '', type: 'separator' },
        { id: 'open-finder', label: 'Open in Finder' },
        { id: 'focus-window', label: 'Open in Focus Window' }
      ]

      if (editors.length > 0) {
        menuItems.push({ id: 'separator-editors', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      menuItems.push(
        { id: 'separator2', label: '', type: 'separator' },
        { id: 'remove', label: 'Remove Workspace' }
      )

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'settings') {
        useSettingsStore.getState().openSettings('projects', projectId)
      } else if (clickedId === 'expand') {
        expand()
      } else if (clickedId === 'open-finder') {
        await invoke('projects_open_in_finder', { path: project.path })
      } else if (clickedId === 'focus-window') {
        await invoke('projects_open_focus_window', { projectId: project.id })
      } else if (clickedId?.startsWith('editor:')) {
        const editorId = clickedId.replace('editor:', '')
        await invoke('projects_open_in_editor', { editorId, path: project.path })
      } else if (clickedId === 'remove') {
        await removeProject(projectId)
      }
    },
    [projects, removeProject, expand]
  )

  return (
    <div
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 gap-1 flex-shrink-0"
      style={{ width: RAIL_WIDTH }}
    >
      {/* Workspace icons */}
      <div className="flex-1 flex flex-col items-center gap-1 overflow-y-auto overflow-x-hidden">
        {orderedProjects.map((project) => (
          <ProjectIcon
            key={project.id}
            project={project}
            isActive={project.id === activeProjectId}
            onClick={() => handleProjectClick(project.id)}
            onContextMenu={(e) => handleContextMenu(e, project.id)}
          />
        ))}
      </div>

      {/* Add workspace button */}
      <button
        className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors"
        onClick={handleAddProject}
        title="Add Workspace"
      >
        <svg
          className="w-4 h-4"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
        </svg>
      </button>

    </div>
  )
}

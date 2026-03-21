import { useState, useCallback, useMemo } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useSettingsStore } from '@/stores/settings'
import { trpc } from '@/lib/trpc'
import { showContextMenu } from '@/lib/context-menu'
import { useGitInfo, useGitChanges } from '@/hooks/useGit'
import ResizeHandle from './ResizeHandle'
import WorktreeDialog from './WorktreeDialog'
import ProjectAvatar from './ProjectAvatar'
import SectionItem from './SectionItem'
import FocusGroupDropdown from './FocusGroupDropdown'

// ── Worktree git badge (shows changed files count) ──────────────────────────

function WorkspaceGitBadge({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span className="ml-auto text-[10px] tabular-nums font-medium px-1.5 py-0.5 bg-yellow-400/10 text-yellow-400 flex-shrink-0">
      {count}
    </span>
  )
}

// ── Worktree status dot (amber pulsing if dirty) ────────────────────────────

function WorkspaceStatusDot({ path }: { path?: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null

  const count = data.changedFiles + data.untrackedFiles
  if (count === 0) return null

  return (
    <span
      className="status-dot-dirty flex-shrink-0"
      style={{
        width: 6,
        height: 6,
        backgroundColor: '#f59e0b'
      }}
    />
  )
}

// ── Diff stats for single-button workspaces ────────────────────────────────────

function DiffStats({ path }: { path: string }): React.JSX.Element | null {
  const { data: changes } = useGitChanges(path)

  const added = changes.filter((f) => f.status === 'added' || f.status === 'untracked').length
  const deleted = changes.filter((f) => f.status === 'deleted').length

  if (added === 0 && deleted === 0) return null

  return (
    <span className="flex items-center gap-1.5 text-[10px] tabular-nums font-medium flex-shrink-0">
      {added > 0 && <span className="text-green-400">+{added}</span>}
      {deleted > 0 && <span className="text-red-400">-{deleted}</span>}
    </span>
  )
}

// ── Aggregated diff stats across all worktrees ──────────────────────────────

function AggregatedDiffStats({
  paths
}: {
  paths: string[]
}): React.JSX.Element | null {
  // We call useGitChanges for each path — this works because the array is stable
  // We render a sub-component per path and aggregate in a wrapper
  return <AggregatedDiffStatsInner paths={paths} />
}

function AggregatedDiffStatsInner({ paths }: { paths: string[] }): React.JSX.Element | null {
  let totalAdded = 0
  let totalDeleted = 0

  for (const path of paths) {
    const { data: changes } = useGitChanges(path)
    totalAdded += changes.filter((f) => f.status === 'added' || f.status === 'untracked').length
    totalDeleted += changes.filter((f) => f.status === 'deleted').length
  }

  if (totalAdded === 0 && totalDeleted === 0) return null

  return (
    <span className="flex items-center gap-1 text-[10px] tabular-nums font-medium flex-shrink-0">
      {totalAdded > 0 && <span className="text-green-400">+{totalAdded}</span>}
      {totalDeleted > 0 && <span className="text-red-400">-{totalDeleted}</span>}
    </span>
  )
}

// ── Ahead/Behind indicators ──────────────────────────────────────────────────

function AheadBehind({ path }: { path: string }): React.JSX.Element | null {
  const { data } = useGitInfo(path)
  if (!data?.isRepo) return null
  if (data.ahead === 0 && data.behind === 0) return null

  return (
    <span className="flex items-center gap-1 text-[10px] tabular-nums font-medium flex-shrink-0">
      {data.ahead > 0 && <span className="text-green-400">{'\u2191'}{data.ahead}</span>}
      {data.behind > 0 && <span className="text-red-400">{'\u2193'}{data.behind}</span>}
    </span>
  )
}

// ── Single Workspace Item (worktreeMode === 0) ─────────────────────────────────

function SingleProjectItem({
  project,
  isActive,
  onContextMenu
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  isActive: boolean
  onContextMenu: (e: React.MouseEvent, projectId: string) => void
}): React.JSX.Element {
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const { data: gitInfo } = useGitInfo(project.path)

  const firstWorkspace = project.workspaces[0]
  const isItemActive = isActive && firstWorkspace && activeWorkspaceId === firstWorkspace.id

  const handleClick = useCallback(() => {
    if (firstWorkspace) {
      setActiveWorkspace(project.id, firstWorkspace.id)
    }
  }, [project.id, firstWorkspace, setActiveWorkspace])

  return (
    <div className="no-drag">
      <button
        className={`w-full flex flex-col gap-0.5 px-3 py-2 text-left text-sm transition-colors ${
          isItemActive
            ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        style={{
          borderLeft: isItemActive ? `2px solid ${project.color}` : '2px solid transparent'
        }}
        onClick={handleClick}
        onContextMenu={(e) => onContextMenu(e, project.id)}
      >
        <div className="flex items-center gap-2 w-full">
          <ProjectAvatar
            projectPath={project.path}
            projectName={project.name}
            projectColor={project.color}
            projectId={project.id}
            iconUrl={project.iconUrl}
            size={20}
            showActivity
          />
          <span className="truncate flex-1">{project.name}</span>
          <AheadBehind path={project.path} />
          <DiffStats path={project.path} />
        </div>
        {gitInfo?.isRepo && gitInfo.currentBranch && (
          <span
            className="text-[10px] font-mono text-[var(--color-text-muted)] truncate pl-7"
            title={gitInfo.currentBranch}
          >
            {gitInfo.currentBranch}
          </span>
        )}
      </button>
    </div>
  )
}

// ── Worktree button (shared between ungrouped and section-grouped) ──────────

function WorkspaceButton({
  workspace,
  projectPath,
  isActive,
  onClick,
  onContextMenu
}: {
  workspace: ReturnType<typeof useProjectsStore.getState>['projects'][number]['workspaces'][number]
  projectPath: string
  isActive: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
}): React.JSX.Element {
  const workspacePath = workspace.worktreePath ?? projectPath
  const isWorktree = workspace.type === 'worktree'
  const branchName = isWorktree ? workspace.branch : undefined

  return (
    <div>
      <button
        className={`w-full flex flex-col gap-0.5 px-3 py-1.5 text-left text-xs transition-colors font-light ${
          isActive
            ? 'bg-white/[0.06] text-[var(--color-accent)]'
            : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
        }`}
        style={{
          borderLeft: isActive ? '2px solid var(--color-accent)' : '2px solid transparent'
        }}
        onClick={onClick}
        onContextMenu={onContextMenu}
      >
        <div className="flex items-center gap-2 w-full">
          <WorkspaceStatusDot path={workspacePath} />

          {isWorktree ? (
            <svg
              className="w-3 h-3 flex-shrink-0 opacity-50"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z"
              />
            </svg>
          ) : (
            <svg
              className="w-3 h-3 flex-shrink-0 opacity-50"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M13 10V3L4 14h7v7l9-11h-7z"
              />
            </svg>
          )}

          <span className="truncate flex-1">{workspace.name}</span>

          <WorkspaceGitBadge path={workspacePath} />
        </div>

        {branchName && (
          <span
            className="text-[10px] font-mono text-[var(--color-text-muted)] truncate pl-5"
            title={branchName}
          >
            {branchName}
          </span>
        )}
      </button>
    </div>
  )
}

// ── Workspace Item (worktreeMode === 1, expandable) ────────────────────────────

function ProjectItem({
  project,
  isActive,
  onContextMenu
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  isActive: boolean
  onContextMenu: (e: React.MouseEvent, projectId: string) => void
}): React.JSX.Element {
  const [isExpanded, setIsExpanded] = useState(isActive)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)
  const assignWorkspaceToSection = useProjectsStore((s) => s.assignWorkspaceToSection)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)

  const handleClick = useCallback(() => {
    setIsExpanded((prev) => !prev)
  }, [])

  const handleWorkspaceClick = useCallback(
    (workspaceId: string) => {
      setActiveWorkspace(project.id, workspaceId)
    },
    [project.id, setActiveWorkspace]
  )

  const handleWorkspaceContextMenu = useCallback(
    async (e: React.MouseEvent, workspaceId: string) => {
      e.preventDefault()
      e.stopPropagation()

      const ws = project.workspaces.find((w) => w.id === workspaceId)
      if (!ws) return

      const sections = project.sections || []
      const isWorktree = ws.type === 'worktree'
      const workspacePath = ws.worktreePath ?? project.path

      // Fetch installed editors for the submenu
      let editors: Array<{ id: string; label: string }> = []
      try {
        editors = await trpc.projects.getEditors.query()
      } catch {
        // ignore
      }

      const menuItems: Array<{ id: string; label: string; type?: string }> = []

      // Open in Finder
      menuItems.push({ id: 'ws-open-finder', label: 'Open in Finder' })

      // Open in Editor submenu
      if (editors.length > 0) {
        menuItems.push({ id: 'ws-editor-sep', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `ws-editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      // Move to Section (only if sections exist)
      if (sections.length > 0) {
        menuItems.push({ id: 'move-header', label: '', type: 'separator' })
        for (const sec of sections) {
          const isCurrent = ws.sectionId === sec.id
          menuItems.push({
            id: `section:${sec.id}`,
            label: `${sec.name}${isCurrent ? ' *' : ''}`
          })
        }
        menuItems.push({ id: 'section:none', label: 'No Section' })
      }

      // Close / Recycle (only for worktree type, not the main branch workspace)
      if (isWorktree) {
        menuItems.push({ id: 'ws-close-sep', label: '', type: 'separator' })
        menuItems.push({ id: 'ws-close', label: 'Close Worktree' })
        menuItems.push({ id: 'ws-recycle', label: 'Recycle Worktree' })
      }

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'ws-open-finder') {
        await trpc.projects.openInFinder.mutate({ path: workspacePath })
      } else if (clickedId?.startsWith('ws-editor:')) {
        const editorId = clickedId.replace('ws-editor:', '')
        await trpc.projects.openInEditor.mutate({ editorId, path: workspacePath })
      } else if (clickedId === 'section:none') {
        await assignWorkspaceToSection(workspaceId, null)
      } else if (clickedId?.startsWith('section:')) {
        const sectionId = clickedId.replace('section:', '')
        await assignWorkspaceToSection(workspaceId, sectionId)
      } else if (clickedId === 'ws-close') {
        // Prevent closing the last worktree
        if (project.workspaces.length <= 1) {
          await showContextMenu([
            { id: 'error', label: 'Cannot close the last worktree' }
          ])
          return
        }
        // Remove from DB only, keep files on disk
        await trpc.workspaces.delete.mutate({ id: workspaceId })
        await fetchProjects()
      } else if (clickedId === 'ws-recycle') {
        // Prevent recycling the last worktree
        if (project.workspaces.length <= 1) {
          await showContextMenu([
            { id: 'error', label: 'Cannot recycle the last worktree' }
          ])
          return
        }
        // Show confirmation via second context menu
        const confirmId = await showContextMenu([
          { id: 'confirm-recycle', label: `Recycle worktree at ${workspacePath}?` },
          { id: 'confirm-sep', label: '', type: 'separator' },
          { id: 'do-recycle', label: 'Confirm Recycle' },
          { id: 'cancel-recycle', label: 'Cancel' }
        ])
        if (confirmId === 'do-recycle') {
          await trpc.git.removeWorktree.mutate({
            worktreePath: workspacePath,
            projectPath: project.path,
            workspaceId: workspaceId
          })
          await fetchProjects()
        }
      }
    },
    [project, assignWorkspaceToSection, fetchProjects]
  )

  // Collect worktree paths for aggregated diff stats
  const workspacePaths = project.workspaces.map(
    (ws) => ws.worktreePath ?? project.path
  )

  // Split worktrees into ungrouped and grouped by section
  const ungroupedWorkspaces = project.workspaces.filter((ws) => !ws.sectionId)
  const sections = project.sections || []

  return (
    <div className="no-drag">
      <button
        className={`w-full flex items-center gap-2 px-3 py-2 text-left text-sm transition-colors ${
          isActive
            ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        onClick={handleClick}
        onContextMenu={(e) => onContextMenu(e, project.id)}
      >
        <ProjectAvatar
          projectPath={project.path}
          projectName={project.name}
          projectColor={project.color}
          projectId={project.id}
          iconUrl={project.iconUrl}
          size={20}
          showActivity
        />
        <span className="truncate flex-1">{project.name}</span>
        <AggregatedDiffStats paths={workspacePaths} />
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
          {project.workspaces.length}
        </span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] transition-transform flex-shrink-0 ${
            isExpanded ? 'rotate-90' : ''
          }`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
      </button>

      {isExpanded && (
        <div className="mt-0.5 mb-0.5">
          {/* Ungrouped worktrees first */}
          {ungroupedWorkspaces.map((workspace) => (
            <WorkspaceButton
              key={workspace.id}
              workspace={workspace}
              projectPath={project.path}
              isActive={activeWorkspaceId === workspace.id}
              onClick={() => handleWorkspaceClick(workspace.id)}
              onContextMenu={(e) => handleWorkspaceContextMenu(e, workspace.id)}
            />
          ))}

          {/* Sections with their worktrees */}
          {sections.map((section) => {
            const sectionWorkspaces = project.workspaces.filter(
              (ws) => ws.sectionId === section.id
            )

            return (
              <SectionItem
                key={section.id}
                section={section}
                workspaces={sectionWorkspaces}
                projectPath={project.path}
                activeWorkspaceId={activeWorkspaceId}
                onWorkspaceClick={handleWorkspaceClick}
                onWorkspaceContextMenu={(e, wsId) => handleWorkspaceContextMenu(e, wsId)}
              />
            )
          })}
        </div>
      )}
    </div>
  )
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

// ── Focus Group Selector ──────────────────────────────────────────────────────

function FocusGroupSelector(): React.JSX.Element {
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)
  const setActiveFocusGroup = useFocusGroupsStore((s) => s.setActiveFocusGroup)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)

  if (!focusGroupsEnabled) {
    return (
      <div className="px-4 pt-3 pb-2 no-drag">
        <span className="text-[11px] font-semibold tracking-wide text-[var(--color-text-muted)] uppercase">
          Workspaces
        </span>
      </div>
    )
  }

  return (
    <div className="px-3 pt-3 pb-2 no-drag">
      <FocusGroupDropdown
        options={focusGroups.map((g) => ({ id: g.id, name: g.name, color: g.color }))}
        value={activeFocusGroupId}
        onChange={setActiveFocusGroup}
      />
    </div>
  )
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

export default function Sidebar(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const addProject = useProjectsStore((s) => s.addProject)
  const renameProject = useProjectsStore((s) => s.renameProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)
  const createSection = useProjectsStore((s) => s.createSection)

  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const activeFocusGroupId = useFocusGroupsStore((s) => s.activeFocusGroupId)

  const filteredProjects = useMemo(() => {
    if (!focusGroupsEnabled || activeFocusGroupId === null) return projects
    return projects.filter((p) => p.focusGroupId === activeFocusGroupId)
  }, [projects, focusGroupsEnabled, activeFocusGroupId])

  // Worktree dialog state
  const [worktreeDialog, setWorktreeDialog] = useState<{
    projectId: string
    projectPath: string
  } | null>(null)

  const handleAddProject = useCallback(async () => {
    const folderPath = await trpc.projects.pickFolder.mutate()
    if (folderPath) {
      await addProject(folderPath)
    }
  }, [addProject])

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, projectId: string) => {
      e.preventDefault()
      const project = projects.find((p) => p.id === projectId)
      if (!project) return

      // Fetch installed editors for the submenu
      let editors: Array<{ id: string; label: string }> = []
      try {
        editors = await trpc.projects.getEditors.query()
      } catch {
        // ignore
      }

      // Build menu items -- add worktree option if worktreeMode is enabled
      const menuItems: Array<{
        id: string
        label: string
        type?: string
      }> = [
        { id: 'settings', label: 'Workspace Settings' },
        { id: 'separator-settings', label: '', type: 'separator' },
        { id: 'rename', label: 'Rename' },
        { id: 'open-finder', label: 'Open in Finder' },
        { id: 'focus-window', label: 'Open in Focus Window' }
      ]

      // Add editor items
      if (editors.length > 0) {
        menuItems.push({ id: 'separator-editors', label: '', type: 'separator' })
        for (const editor of editors) {
          menuItems.push({ id: `editor:${editor.id}`, label: `Open in ${editor.label}` })
        }
      }

      if (project.worktreeMode) {
        menuItems.push(
          { id: 'separator-wt', label: '', type: 'separator' },
          { id: 'new-worktree', label: 'New Worktree...' },
          { id: 'new-section', label: 'New Section...' }
        )
      }

      // Worktree mode toggle
      menuItems.push(
        { id: 'separator-toggle', label: '', type: 'separator' },
        {
          id: 'toggle-worktree-mode',
          label: project.worktreeMode ? 'Disable Worktrees' : 'Enable Worktrees'
        }
      )

      menuItems.push(
        { id: 'separator', label: '', type: 'separator' },
        { id: 'remove', label: 'Remove Workspace' }
      )

      const clickedId = await showContextMenu(menuItems)

      if (clickedId === 'settings') {
        useSettingsStore.getState().openSettings()
        useSettingsStore.getState().setSection('projects')
      } else if (clickedId === 'rename') {
        const newName = window.prompt('Rename workspace:', project.name)
        if (newName && newName.trim() && newName !== project.name) {
          await renameProject(projectId, newName.trim())
        }
      } else if (clickedId === 'open-finder') {
        await trpc.projects.openInFinder.mutate({ path: project.path })
      } else if (clickedId === 'focus-window') {
        await trpc.projects.openFocusWindow.mutate({ projectId: project.id })
      } else if (clickedId?.startsWith('editor:')) {
        const editorId = clickedId.replace('editor:', '')
        await trpc.projects.openInEditor.mutate({ editorId, path: project.path })
      } else if (clickedId === 'new-worktree') {
        setWorktreeDialog({ projectId: project.id, projectPath: project.path })
      } else if (clickedId === 'new-section') {
        const sectionName = window.prompt('Section name:')
        if (sectionName && sectionName.trim()) {
          await createSection(project.id, sectionName.trim())
        }
      } else if (clickedId === 'toggle-worktree-mode') {
        const newMode = project.worktreeMode ? 0 : 1
        await trpc.projects.update.mutate({ id: projectId, worktreeMode: newMode })
        await fetchProjects()
      } else if (clickedId === 'remove') {
        await removeProject(projectId)
      }
    },
    [projects, removeProject, renameProject, fetchProjects, createSection]
  )

  return (
    <div className="relative flex flex-col h-full">
      <ResizeHandle />

      {/* Focus Group Selector (replaces branding area) */}
      <FocusGroupSelector />

      {/* Workspace list */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        {filteredProjects.map((project, idx) => (
          <div key={project.id}>
            {project.worktreeMode === 0 ? (
              <SingleProjectItem
                project={project}
                isActive={project.id === activeProjectId}
                onContextMenu={handleContextMenu}
              />
            ) : (
              <ProjectItem
                project={project}
                isActive={project.id === activeProjectId}
                onContextMenu={handleContextMenu}
              />
            )}
            {idx < filteredProjects.length - 1 && (
              <div className="border-b border-[var(--color-border)]" />
            )}
          </div>
        ))}

        {filteredProjects.length === 0 && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">
              {projects.length === 0 ? 'No workspaces yet' : 'No workspaces in this group'}
            </p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-60">
              {projects.length === 0 ? 'Add a folder to get started' : 'Assign workspaces in Settings'}
            </p>
          </div>
        )}
      </div>

      {/* Add Workspace button */}
      <div className="p-3 border-t border-[var(--color-border)]">
        <button
          className="no-drag w-full flex items-center justify-center gap-2 px-3 py-2 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] transition-colors"
          onClick={handleAddProject}
        >
          <svg
            className="w-3.5 h-3.5"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
          Add Workspace
        </button>
      </div>

      {/* Worktree creation dialog */}
      {worktreeDialog && (
        <WorktreeDialog
          projectId={worktreeDialog.projectId}
          projectPath={worktreeDialog.projectPath}
          open={true}
          onClose={() => setWorktreeDialog(null)}
        />
      )}
    </div>
  )
}

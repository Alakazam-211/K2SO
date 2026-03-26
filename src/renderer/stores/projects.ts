import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useGitInitDialogStore } from './git-init-dialog'
import { useToastStore } from './toast'
import { useTabsStore } from './tabs'

// Debounce touchInteraction to avoid excessive DB writes (5 min per project)
const TOUCH_DEBOUNCE_MS = 5 * 60 * 1000
const _lastTouchMap = new Map<string, number>()

interface Workspace {
  id: string
  projectId: string
  sectionId: string | null
  type: string
  branch: string | null
  name: string
  tabOrder: number
  worktreePath: string | null
  createdAt: number
}

export interface WorkspaceSection {
  id: string
  projectId: string
  name: string
  color: string | null
  isCollapsed: number
  tabOrder: number
  createdAt: number
}

interface Project {
  id: string
  name: string
  path: string
  color: string
  tabOrder: number
  lastOpenedAt: number | null
  worktreeMode: number
  iconUrl: string | null
  focusGroupId: string | null
  pinned: number
  manuallyActive: number
  lastInteractionAt: number | null
  createdAt: number
  agentEnabled: number
  heartbeatEnabled: number
}

export interface ProjectWithWorkspaces extends Project {
  workspaces: Workspace[]
  sections: WorkspaceSection[]
}

interface ProjectsState {
  projects: ProjectWithWorkspaces[]
  activeProjectId: string | null
  activeWorkspaceId: string | null

  fetchProjects: () => Promise<void>
  addProject: (path: string) => Promise<void>
  removeProject: (id: string) => Promise<void>
  setActiveProject: (id: string | null) => void
  setActiveWorkspace: (projectId: string, workspaceId: string) => void
  reorderProjects: (ids: string[]) => Promise<void>
  renameProject: (id: string, name: string) => Promise<void>
  createSection: (projectId: string, name: string, color?: string) => Promise<void>
  deleteSection: (id: string) => Promise<void>
  renameSection: (id: string, name: string) => Promise<void>
  updateSection: (id: string, updates: { name?: string; color?: string | null; isCollapsed?: number }) => Promise<void>
  assignWorkspaceToSection: (workspaceId: string, sectionId: string | null) => Promise<void>
  touchInteraction: (projectId: string) => void
  setManuallyActive: (projectId: string, active: boolean) => Promise<void>
}

export const useProjectsStore = create<ProjectsState>((set, get) => ({
  projects: [],
  activeProjectId: null,
  activeWorkspaceId: null,

  fetchProjects: async () => {
    try {
      const projectList = await invoke<Project[]>('projects_list')

      // Fetch workspaces and sections for each project
      const projectsWithWorkspaces: ProjectWithWorkspaces[] = await Promise.all(
        projectList.map(async (project: Project) => {
          const ws = await invoke<Workspace[]>('workspaces_list', { projectId: project.id })
          let secs: WorkspaceSection[] = []
          try {
            secs = await invoke<WorkspaceSection[]>('sections_list', { projectId: project.id })
          } catch {
            // sections table might not exist yet
          }
          return {
            ...project,
            workspaces: ws,
            sections: secs
          }
        })
      )

      set({ projects: projectsWithWorkspaces })

      // If active project was deleted (e.g. in another window), clear selection
      const state = get()
      if (state.activeProjectId && !projectsWithWorkspaces.find((p) => p.id === state.activeProjectId)) {
        if (projectsWithWorkspaces.length > 0) {
          const first = projectsWithWorkspaces[0]
          set({ activeProjectId: first.id, activeWorkspaceId: first.workspaces[0]?.id ?? null })
        } else {
          set({ activeProjectId: null, activeWorkspaceId: null })
        }
      }

      // If we have projects but no active one, select the first
      if (!get().activeProjectId && projectsWithWorkspaces.length > 0) {
        const firstProject = projectsWithWorkspaces[0]
        const firstWorkspaceId = firstProject.workspaces[0]?.id ?? null
        set({
          activeProjectId: firstProject.id,
          activeWorkspaceId: firstWorkspaceId
        })

        // Load saved layout for the initial workspace (if any)
        if (firstWorkspaceId) {
          const cwd = firstProject.workspaces[0]?.worktreePath ?? firstProject.path ?? '~'
          useTabsStore.getState().loadLayoutForWorkspace(firstProject.id, firstWorkspaceId, cwd)
        }
      }
    } catch (err) {
      console.error('[projects] fetchProjects failed:', err)
    }
  },

  addProject: async (path: string) => {
    try {
      const result = await invoke<{ needsGitInit?: true; path: string; name: string } | null>('projects_add_from_path', { path })
      console.log('[projects] addFromPath result:', JSON.stringify(result))

      // Check if the folder needs git initialization
      if (result && typeof result === 'object' && 'needsGitInit' in result) {
        const r = result as { needsGitInit: true; path: string; name: string }
        console.log('[projects] Opening git init dialog for:', r.path)
        useGitInitDialogStore.getState().open(r.path, r.name)
        return
      }

      // Stash the workspace we're leaving BEFORE fetchProjects changes active IDs
      const preState = get()
      const tabsStore = useTabsStore.getState()
      if (preState.activeProjectId && preState.activeWorkspaceId) {
        tabsStore.stashWorkspace(`${preState.activeProjectId}:${preState.activeWorkspaceId}`)
      }

      await get().fetchProjects()

      const state = get()
      const newProject = state.projects[state.projects.length - 1]
      if (newProject) {
        const newWorkspaceId = newProject.workspaces[0]?.id ?? null
        set({
          activeProjectId: newProject.id,
          activeWorkspaceId: newWorkspaceId
        })

        if (newWorkspaceId) {
          const cwd = newProject.workspaces[0]?.worktreePath ?? newProject.path ?? '~'
          const newKey = `${newProject.id}:${newWorkspaceId}`
          tabsStore.restoreWorkspace(newKey, cwd)
        } else {
          tabsStore.clearAllTabs()
        }
      }

      useToastStore.getState().addToast('Workspace added', 'success')
    } catch (err) {
      console.error('[projects] addProject failed:', err)
      useToastStore.getState().addToast('Failed to add workspace', 'error')
    }
  },

  removeProject: async (id: string) => {
    try {
      const tabsStore = useTabsStore.getState()

      // Clean up background workspaces for this project
      for (const key of Object.keys(tabsStore.backgroundWorkspaces)) {
        if (key.startsWith(`${id}:`)) {
          tabsStore.clearBackgroundWorkspace(key)
        }
      }
      // Delete saved sessions from DB
      invoke('workspace_session_delete', { projectId: id, workspaceId: null }).catch((e) => console.warn('[projects] workspace_session_delete failed:', e))

      await invoke('projects_delete', { id })

      const state = get()
      if (state.activeProjectId === id) {
        tabsStore.clearAllTabs()
        set({ activeProjectId: null, activeWorkspaceId: null })
      }

      await get().fetchProjects()

      // Select first remaining project if active was deleted
      const updated = get()
      if (!updated.activeProjectId && updated.projects.length > 0) {
        const first = updated.projects[0]
        const firstWorkspaceId = first.workspaces[0]?.id ?? null
        set({
          activeProjectId: first.id,
          activeWorkspaceId: firstWorkspaceId
        })

        if (firstWorkspaceId) {
          const cwd = first.workspaces[0]?.worktreePath ?? first.path ?? '~'
          const newKey = `${first.id}:${firstWorkspaceId}`
          tabsStore.restoreWorkspace(newKey, cwd)
        }
      }

      useToastStore.getState().addToast('Workspace removed', 'info')
    } catch (err) {
      console.error('[projects] removeProject failed:', err)
    }
  },

  setActiveProject: (id: string | null) => {
    const state = get()
    const tabsStore = useTabsStore.getState()

    // Stash current workspace (PTYs stay alive in background)
    if (state.activeProjectId && state.activeWorkspaceId) {
      tabsStore.stashWorkspace(`${state.activeProjectId}:${state.activeWorkspaceId}`)
    }

    if (id === null) {
      set({ activeProjectId: null, activeWorkspaceId: null })
      return
    }

    const project = state.projects.find((p) => p.id === id)
    if (project) {
      const newWorkspaceId = project.workspaces[0]?.id ?? null
      set({
        activeProjectId: id,
        activeWorkspaceId: newWorkspaceId
      })

      if (newWorkspaceId) {
        const cwd = project.workspaces[0]?.worktreePath ?? project.path ?? '~'
        const newKey = `${id}:${newWorkspaceId}`
        tabsStore.restoreWorkspace(newKey, cwd)
      }
    }
  },

  setActiveWorkspace: (projectId: string, workspaceId: string) => {
    const state = get()
    const tabsStore = useTabsStore.getState()

    // Stash current workspace (PTYs stay alive in background)
    if (state.activeProjectId && state.activeWorkspaceId) {
      tabsStore.stashWorkspace(`${state.activeProjectId}:${state.activeWorkspaceId}`)
    }

    set({
      activeProjectId: projectId,
      activeWorkspaceId: workspaceId
    })

    const project = state.projects.find((p) => p.id === projectId)
    const workspace = project?.workspaces.find((w) => w.id === workspaceId)
    const cwd = workspace?.worktreePath ?? project?.path ?? '~'
    const newKey = `${projectId}:${workspaceId}`
    tabsStore.restoreWorkspace(newKey, cwd)
  },

  reorderProjects: async (ids: string[]) => {
    try {
      await invoke('projects_reorder', { ids })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] reorderProjects failed:', err)
    }
  },

  renameProject: async (id: string, name: string) => {
    try {
      await invoke('projects_update', { id, name })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameProject failed:', err)
    }
  },

  createSection: async (projectId: string, name: string, color?: string) => {
    try {
      await invoke('sections_create', { projectId, name, color })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] createSection failed:', err)
    }
  },

  deleteSection: async (id: string) => {
    try {
      await invoke('sections_delete', { id })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] deleteSection failed:', err)
    }
  },

  renameSection: async (id: string, name: string) => {
    try {
      await invoke('sections_update', { id, name })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameSection failed:', err)
    }
  },

  updateSection: async (id: string, updates: { name?: string; color?: string | null; isCollapsed?: number }) => {
    try {
      await invoke('sections_update', { id, ...updates })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] updateSection failed:', err)
    }
  },

  assignWorkspaceToSection: async (workspaceId: string, sectionId: string | null) => {
    try {
      await invoke('sections_assign_workspace', { workspaceId, sectionId })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] assignWorkspaceToSection failed:', err)
    }
  },

  touchInteraction: async (projectId: string) => {
    const now = Date.now()
    const last = _lastTouchMap.get(projectId) || 0
    if (now - last < TOUCH_DEBOUNCE_MS) return
    _lastTouchMap.set(projectId, now)
    // Optimistic local update
    set((state) => ({
      projects: state.projects.map((p) =>
        p.id === projectId ? { ...p, lastInteractionAt: Math.floor(now / 1000) } : p
      )
    }))
    // Write to DB — await so subsequent fetchProjects picks up the value
    try {
      await invoke('projects_touch_interaction', { id: projectId })
    } catch (err) {
      console.warn('[projects] touchInteraction failed:', err)
    }
  },

  setManuallyActive: async (projectId: string, active: boolean) => {
    try {
      await invoke('projects_update', { id: projectId, manuallyActive: active ? 1 : 0 })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] setManuallyActive failed:', err)
    }
  }
}))

// Initialize on import
useProjectsStore.getState().fetchProjects()

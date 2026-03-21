import { create } from 'zustand'
import { trpc } from '@/lib/trpc'
import { useGitInitDialogStore } from './git-init-dialog'

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
  createdAt: number
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
}

export const useProjectsStore = create<ProjectsState>((set, get) => ({
  projects: [],
  activeProjectId: null,
  activeWorkspaceId: null,

  fetchProjects: async () => {
    try {
      const projectList = await trpc.projects.list.query()

      // Fetch workspaces and sections for each project
      const projectsWithWorkspaces: ProjectWithWorkspaces[] = await Promise.all(
        projectList.map(async (project: Project) => {
          const ws = await trpc.workspaces.list.query({ projectId: project.id })
          let secs: WorkspaceSection[] = []
          try {
            secs = (await trpc.sections.list.query({ projectId: project.id })) as WorkspaceSection[]
          } catch {
            // sections table might not exist yet
          }
          return {
            ...project,
            workspaces: ws as Workspace[],
            sections: secs
          }
        })
      )

      set({ projects: projectsWithWorkspaces })

      // If we have projects but no active one, select the first
      const state = get()
      if (!state.activeProjectId && projectsWithWorkspaces.length > 0) {
        const firstProject = projectsWithWorkspaces[0]
        set({
          activeProjectId: firstProject.id,
          activeWorkspaceId: firstProject.workspaces[0]?.id ?? null
        })
      }
    } catch (err) {
      console.error('[projects] fetchProjects failed:', err)
    }
  },

  addProject: async (path: string) => {
    try {
      const result = await trpc.projects.addFromPath.mutate({ path })

      // Check if the folder needs git initialization
      if (result && typeof result === 'object' && 'needsGitInit' in result && result.needsGitInit) {
        const r = result as { needsGitInit: true; path: string; name: string }
        useGitInitDialogStore.getState().open(r.path, r.name)
        return
      }

      await get().fetchProjects()

      // Set the newly added project as active
      const state = get()
      const newProject = state.projects[state.projects.length - 1]
      if (newProject) {
        set({
          activeProjectId: newProject.id,
          activeWorkspaceId: newProject.workspaces[0]?.id ?? null
        })
      }
    } catch (err) {
      console.error('[projects] addProject failed:', err)
    }
  },

  removeProject: async (id: string) => {
    try {
      await trpc.projects.delete.mutate({ id })

      const state = get()
      if (state.activeProjectId === id) {
        set({ activeProjectId: null, activeWorkspaceId: null })
      }

      await get().fetchProjects()

      // Select first remaining project if active was deleted
      const updated = get()
      if (!updated.activeProjectId && updated.projects.length > 0) {
        const first = updated.projects[0]
        set({
          activeProjectId: first.id,
          activeWorkspaceId: first.workspaces[0]?.id ?? null
        })
      }
    } catch (err) {
      console.error('[projects] removeProject failed:', err)
    }
  },

  setActiveProject: (id: string | null) => {
    if (id === null) {
      set({ activeProjectId: null, activeWorkspaceId: null })
      return
    }

    const project = get().projects.find((p) => p.id === id)
    if (project) {
      set({
        activeProjectId: id,
        activeWorkspaceId: project.workspaces[0]?.id ?? null
      })
    }
  },

  setActiveWorkspace: (projectId: string, workspaceId: string) => {
    set({
      activeProjectId: projectId,
      activeWorkspaceId: workspaceId
    })
  },

  reorderProjects: async (ids: string[]) => {
    try {
      await trpc.projects.reorder.mutate({ ids })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] reorderProjects failed:', err)
    }
  },

  renameProject: async (id: string, name: string) => {
    try {
      await trpc.projects.update.mutate({ id, name })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameProject failed:', err)
    }
  },

  createSection: async (projectId: string, name: string, color?: string) => {
    try {
      await trpc.sections.create.mutate({ projectId, name, color })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] createSection failed:', err)
    }
  },

  deleteSection: async (id: string) => {
    try {
      await trpc.sections.delete.mutate({ id })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] deleteSection failed:', err)
    }
  },

  renameSection: async (id: string, name: string) => {
    try {
      await trpc.sections.update.mutate({ id, name })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] renameSection failed:', err)
    }
  },

  updateSection: async (id: string, updates: { name?: string; color?: string | null; isCollapsed?: number }) => {
    try {
      await trpc.sections.update.mutate({ id, ...updates })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] updateSection failed:', err)
    }
  },

  assignWorkspaceToSection: async (workspaceId: string, sectionId: string | null) => {
    try {
      await trpc.sections.assignWorkspace.mutate({ workspaceId, sectionId })
      await get().fetchProjects()
    } catch (err) {
      console.error('[projects] assignWorkspaceToSection failed:', err)
    }
  }
}))

// Initialize on import
useProjectsStore.getState().fetchProjects()

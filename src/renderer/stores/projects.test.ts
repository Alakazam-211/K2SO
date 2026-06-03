// Plan B (Bulk-1) — vitest coverage for the projects store after migrating
// its DB-backed actions OFF the Tauri `projects_*`/`workspaces_*`/`sections_*`
// invoke proxy ONTO the host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchProjects   → GET `projects/list` + per-project `workspaces/list`
//                       and `sections/list` with snake_case `project_id`
//   - renameProject   → POST `projects/update`  + emits sync:projects
//   - reorderProjects → POST `projects/reorder` + emits sync:projects
//   - removeProject   → POST `workspace-layouts/delete` + `projects/delete`
//                       + emits sync:projects
//   - createSection   → POST `sections/create`  (NO sync — old shim emitted none)
//   - assignWorkspaceToSection → POST `sections/assign` (camelCase, no sync)
//   - setManuallyActive → POST `projects/update` + emits sync:projects
//   - touchInteraction  → POST `projects/touch-interaction` (NO sync)
//
// The store has an import-time side effect (`fetchProjects()`), so every
// dependency is mocked via hoisted `vi.mock` BEFORE the store import.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Mock the host-aware daemon-cli layer (the thing we migrated TO) ──────
const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliGetText: vi.fn(),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

// ── Mock the cross-window emit bus ───────────────────────────────────────
const emitMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => emitMock(...args),
}))

// ── Mock daemon-settings (fetchProjects' last-session restore reads it) ──
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => Promise.resolve({})),
  settingsUpdate: vi.fn(() => Promise.resolve({})),
}))

// ── Mock the daemon-reconnect bus (no-op listener registration) ──────────
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// ── Cross-store deps (only their shapes matter for these paths) ──────────
vi.mock('./git-init-dialog', () => ({
  useGitInitDialogStore: { getState: () => ({ open: vi.fn() }) },
}))
const addToast = vi.fn()
vi.mock('./toast', () => ({
  useToastStore: { getState: () => ({ addToast }) },
}))
vi.mock('./tabs', () => ({
  useTabsStore: {
    getState: () => ({
      stashWorkspace: vi.fn(),
      clearAllTabs: vi.fn(),
      restoreWorkspace: vi.fn(),
      loadLayoutForWorkspace: vi.fn(),
      clearBackgroundWorkspace: vi.fn(),
      tabs: [],
      backgroundWorkspaces: {},
    }),
  },
  ensurePinnedAgentTabForMode: vi.fn(),
}))
vi.mock('./focus-groups', () => ({
  useFocusGroupsStore: {
    getState: () => ({ focusGroupsEnabled: false, activeFocusGroupId: null }),
  },
}))
vi.mock('./settings', () => ({
  useSettingsStore: {
    getState: () => ({ loaded: true, lastActiveProjectId: null, lastActiveWorkspaceId: null }),
  },
}))

import { useProjectsStore, type ProjectWithWorkspaces } from './projects'

function mkProject(id: string): Record<string, unknown> {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    color: '#fff',
    tabOrder: 0,
    lastOpenedAt: null,
    worktreeMode: 0,
    iconUrl: null,
    focusGroupId: null,
    pinned: 0,
    manuallyActive: 0,
    lastInteractionAt: null,
    createdAt: 1,
    agentEnabled: 0,
    heartbeatEnabled: 0,
    agentMode: 'off',
    stateId: null,
    heartbeatMode: 'off',
    heartbeatSchedule: null,
    heartbeatLastFire: null,
  }
}

function resetStore(): void {
  useProjectsStore.setState({ projects: [], activeProjectId: null, activeWorkspaceId: null })
}

describe('projects store — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    emitMock.mockClear()
    addToast.mockClear()
    resetStore()
  })

  it('fetchProjects GETs projects/list then workspaces/list + sections/list per project (snake_case)', async () => {
    daemonCliGet.mockImplementation((route: string, params?: Record<string, unknown>) => {
      if (route === 'projects/list') return Promise.resolve([mkProject('p1')])
      if (route === 'workspaces/list') {
        expect(params).toEqual({ project_id: 'p1' })
        return Promise.resolve([{ id: 'w1', projectId: 'p1', sectionId: null, type: 'main', branch: null, name: 'main', tabOrder: 0, worktreePath: null, navVisible: 1, createdAt: 1 }])
      }
      if (route === 'sections/list') {
        expect(params).toEqual({ project_id: 'p1' })
        return Promise.resolve([])
      }
      throw new Error(`unexpected GET ${route}`)
    })

    await useProjectsStore.getState().fetchProjects()

    expect(daemonCliGet).toHaveBeenCalledWith('projects/list')
    expect(daemonCliGet).toHaveBeenCalledWith('workspaces/list', { project_id: 'p1' })
    expect(daemonCliGet).toHaveBeenCalledWith('sections/list', { project_id: 'p1' })

    const projects = useProjectsStore.getState().projects as ProjectWithWorkspaces[]
    expect(projects).toHaveLength(1)
    expect(projects[0].id).toBe('p1')
    expect(projects[0].workspaces).toHaveLength(1)
    expect(projects[0].sections).toEqual([])
  })

  it('renameProject POSTs projects/update (camelCase) and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([]) // the refetch

    await useProjectsStore.getState().renameProject('p1', 'New Name')

    expect(daemonCliPost).toHaveBeenCalledWith('projects/update', { id: 'p1', name: 'New Name' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('reorderProjects POSTs projects/reorder and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().reorderProjects(['a', 'b'])

    expect(daemonCliPost).toHaveBeenCalledWith('projects/reorder', { ids: ['a', 'b'] })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('removeProject POSTs workspace-layouts/delete + projects/delete and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValue({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().removeProject('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('workspace-layouts/delete', { projectId: 'p1', workspaceId: null })
    expect(daemonCliPost).toHaveBeenCalledWith('projects/delete', { id: 'p1' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('createSection POSTs sections/create and does NOT emit sync (old shim emitted none)', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().createSection('p1', 'Group', '#abc')

    expect(daemonCliPost).toHaveBeenCalledWith('sections/create', { projectId: 'p1', name: 'Group', color: '#abc' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  it('assignWorkspaceToSection POSTs sections/assign (camelCase) without sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().assignWorkspaceToSection('w1', 'sec1')

    expect(daemonCliPost).toHaveBeenCalledWith('sections/assign', { workspaceId: 'w1', sectionId: 'sec1' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  it('setManuallyActive POSTs projects/update and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().setManuallyActive('p1', true)

    expect(daemonCliPost).toHaveBeenCalledWith('projects/update', { id: 'p1', manuallyActive: 1 })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('touchInteraction POSTs projects/touch-interaction and does NOT emit sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })

    await useProjectsStore.getState().touchInteraction('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('projects/touch-interaction', { id: 'p1' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  it('a failed mutation does NOT emit sync', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('daemon down'))
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().renameProject('p1', 'X')

    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })
})

import React from 'react'
import { useEffect, useState, useCallback, useRef, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import IconCropDialog from '../IconCropDialog'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { SettingsGroup, SettingDropdown } from '../controls/SettingControls'
import { CAP_LABELS, CAP_COLORS, CAPABILITIES, type StateData } from '@shared/constants/capabilities'
import { showContextMenu } from '@/lib/context-menu'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import type { SettingEntry } from '../searchManifest'
import { HeartbeatsPanel, HistoryPanel, WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'
import { ContextLayersPreview } from './ContextLayersPreview'

export const PROJECTS_MANIFEST: SettingEntry[] = [
  { id: 'projects.list', section: 'projects', label: 'Workspaces', description: 'All registered projects + focus groups', keywords: ['workspaces', 'projects', 'focus groups'] },
  { id: 'projects.add', section: 'projects', label: 'Add Workspace', description: 'Register a new project directory', keywords: ['add', 'new', 'workspace', 'project', 'folder'] },
  { id: 'projects.focus-groups', section: 'projects', label: 'Focus Groups', description: 'Organize workspaces into tabbed folders', keywords: ['focus', 'groups', 'tabs'] },
  { id: 'projects.project-context', section: 'projects', label: 'Project Context', description: 'Shared .k2so/PROJECT.md injected into every agent', keywords: ['project context', 'project.md', 'claude.md', 'shared'] },
  { id: 'projects.heartbeat', section: 'projects', label: 'Heartbeat Schedule', description: 'Scheduled / hourly / off per-project heartbeat mode', keywords: ['heartbeat', 'schedule', 'cron', 'hourly', 'scheduled'] },
  { id: 'projects.agents', section: 'projects', label: 'Project Agents', description: 'Custom agent personas + wake-up files per workspace', keywords: ['agent', 'persona', 'wakeup', 'create'] },
  { id: 'projects.worktrees', section: 'projects', label: 'Worktree Folders', description: 'Enable/disable per-agent git worktrees', keywords: ['worktree', 'git', 'branch'] },
  { id: 'projects.relations', section: 'projects', label: 'Connected Workspaces', description: 'Workspace relations for cross-project messaging', keywords: ['relations', 'connected', 'cross-workspace', 'links'] },
  { id: 'projects.cursor-migrate', section: 'projects', label: 'Cursor Session Migration', description: 'Port Cursor IDE sessions into K2SO', keywords: ['cursor', 'migrate', 'session', 'import'] },
]

export function ProjectsSection(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const removeProject = useProjectsStore((s) => s.removeProject)
  const fetchProjects = useProjectsStore((s) => s.fetchProjects)
  const projectSettings = useSettingsStore((s) => s.projectSettings)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)

  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const setFocusGroupsEnabled = useFocusGroupsStore((s) => s.setFocusGroupsEnabled)
  const createFocusGroup = useFocusGroupsStore((s) => s.createFocusGroup)
  const deleteFocusGroup = useFocusGroupsStore((s) => s.deleteFocusGroup)
  const renameFocusGroup = useFocusGroupsStore((s) => s.renameFocusGroup)
  const assignProjectToGroup = useFocusGroupsStore((s) => s.assignProjectToGroup)

  const initialProjectId = useSettingsStore((s) => s.initialProjectId)
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(
    initialProjectId ?? (projects.length > 0 ? projects[0].id : null)
  )

  // When initialProjectId changes (e.g. right-click a different project), update selection
  useEffect(() => {
    if (initialProjectId) {
      setSelectedProjectId(initialProjectId)
    }
  }, [initialProjectId])

  const [newGroupName, setNewGroupName] = useState('')
  const [searchQuery, setSearchQuery] = useState('')
  const searchInputRef = useRef<HTMLInputElement>(null)
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set())
  const [dragProjectId, setDragProjectId] = useState<string | null>(null)
  const [dragOverGroupId, setDragOverGroupId] = useState<string | null>(null)

  // ── Focus group reorder state ──
  const [groupDragId, setGroupDragId] = useState<string | null>(null)
  const [groupDropIndex, setGroupDropIndex] = useState<number | null>(null)
  const groupDragIdRef = useRef<string | null>(null)
  const groupDropRef = useRef<number | null>(null)
  const reorderFocusGroups = useFocusGroupsStore((s) => s.reorderFocusGroups)
  const [renamingGroupId, setRenamingGroupId] = useState<string | null>(null)
  const [renamingGroupName, setRenamingGroupName] = useState('')
  const renameGroupInputRef = useRef<HTMLInputElement>(null)

  const handleGroupContextMenu = useCallback(async (e: React.MouseEvent, groupId: string) => {
    e.preventDefault()
    e.stopPropagation()
    const group = focusGroups.find((g) => g.id === groupId)
    if (!group) return

    const clickedId = await showContextMenu([
      { id: 'rename', label: 'Rename' },
      { id: 'delete', label: 'Delete' },
    ])

    if (clickedId === 'rename') {
      setRenamingGroupId(groupId)
      setRenamingGroupName(group.name)
      requestAnimationFrame(() => renameGroupInputRef.current?.focus())
    } else if (clickedId === 'delete') {
      await deleteFocusGroup(groupId)
      await fetchProjects()
    }
  }, [focusGroups, deleteFocusGroup, fetchProjects])

  const handleGroupRenameConfirm = useCallback(async () => {
    if (renamingGroupId && renamingGroupName.trim()) {
      await renameFocusGroup(renamingGroupId, renamingGroupName.trim())
    }
    setRenamingGroupId(null)
    setRenamingGroupName('')
  }, [renamingGroupId, renamingGroupName, renameFocusGroup])

  const handleGroupReorderMouseDown = useCallback((e: React.MouseEvent, groupId: string) => {
    if (e.button !== 0) return
    // Don't start drag from interactive elements
    if ((e.target as HTMLElement).closest('button, input')) return
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 5) {
        started = true
        groupDragIdRef.current = groupId
        setGroupDragId(groupId)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      const container = document.querySelector('[data-focus-group-reorder-container]')
      if (!container) return
      const items = container.querySelectorAll('[data-focus-group-reorder-id]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) dropIdx = i + 1
      }
      groupDropRef.current = dropIdx
      setGroupDropIndex(dropIdx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        const dragId = groupDragIdRef.current
        const dropIdx = groupDropRef.current
        if (dragId && dropIdx !== null) {
          const currentGroups = useFocusGroupsStore.getState().focusGroups
          const fromIdx = currentGroups.findIndex((g) => g.id === dragId)
          if (fromIdx >= 0 && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
            const list = [...currentGroups]
            const [moved] = list.splice(fromIdx, 1)
            const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
            list.splice(insertAt, 0, moved)
            await reorderFocusGroups(list.map((g) => g.id))
          }
        }
      }

      setGroupDragId(null)
      setGroupDropIndex(null)
      groupDragIdRef.current = null
      groupDropRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderFocusGroups])

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null
  const editors = ['Cursor', 'VS Code', 'Zed', 'Other']

  const toggleGroupCollapse = useCallback((groupId: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev)
      if (next.has(groupId)) next.delete(groupId)
      else next.add(groupId)
      return next
    })
  }, [])

  const handleCreateGroup = useCallback(async () => {
    if (!newGroupName.trim()) return
    await createFocusGroup(newGroupName.trim())
    setNewGroupName('')
  }, [newGroupName, createFocusGroup])

  const handleDrop = useCallback(async (groupId: string | null) => {
    if (!dragProjectId) return
    await assignProjectToGroup(dragProjectId, groupId)
    await fetchProjects()
    setDragProjectId(null)
    setDragOverGroupId(null)
  }, [dragProjectId, assignProjectToGroup, fetchProjects])

  // ── Reorder state ──────────────────────────────────────────────────
  const [reorderDragId, setReorderDragId] = useState<string | null>(null)
  const [reorderDropIndex, setReorderDropIndex] = useState<number | null>(null)
  const [reorderZone, setReorderZone] = useState<string | null>(null)
  const reorderDropRef = useRef<number | null>(null)
  const reorderZoneRef = useRef<string | null>(null)
  const dragOverGroupRef = useRef<string | null>(null)

  // Auto-focus search when navigating to Workspaces page
  useEffect(() => {
    requestAnimationFrame(() => searchInputRef.current?.focus())
  }, [])

  const settingsAgenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  // Filter helper for search
  const matchesSearch = useCallback((p: typeof projects[0]) => {
    if (!searchQuery.trim()) return true
    const q = searchQuery.toLowerCase()
    return p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q)
  }, [searchQuery])

  const agentPinnedProjects = useMemo(() =>
    settingsAgenticEnabled ? projects.filter((p) => (p.agentMode === 'agent' || p.agentMode === 'custom') && matchesSearch(p)) : [],
    [projects, settingsAgenticEnabled, matchesSearch])
  const agentIds = useMemo(() => new Set(
    (settingsAgenticEnabled ? projects.filter((p) => p.agentMode === 'agent' || p.agentMode === 'custom') : []).map((p) => p.id)
  ), [projects, settingsAgenticEnabled])
  const pinnedProjects = useMemo(() => projects.filter((p) => p.pinned && !agentIds.has(p.id) && matchesSearch(p)), [projects, agentIds, matchesSearch])
  const regularPinnedProjects = pinnedProjects
  const ungroupedProjects = projects.filter((p) => !p.focusGroupId && !p.pinned && !agentIds.has(p.id) && matchesSearch(p))
  const reorderProjects = useProjectsStore((s) => s.reorderProjects)

  const handleReorderMouseDown = useCallback((
    e: React.MouseEvent,
    projectId: string,
    zone: string,
    containerSelector: string
  ) => {
    if (e.button !== 0) return
    const startX = e.clientX
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        setReorderDragId(projectId)
        setReorderZone(zone)
        reorderZoneRef.current = zone
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      // Check if hovering over a focus group header
      const el = document.elementFromPoint(ev.clientX, ev.clientY)
      const groupHeader = el?.closest('[data-focus-group-id]') as HTMLElement | null
      if (groupHeader) {
        const gid = groupHeader.dataset.focusGroupId!
        dragOverGroupRef.current = gid
        setDragOverGroupId(gid)
        setReorderDropIndex(null)
        reorderDropRef.current = null
        return
      } else {
        dragOverGroupRef.current = null
        setDragOverGroupId(null)
      }

      // Check within-zone reorder
      const container = document.querySelector(containerSelector)
      if (!container) return
      const items = container.querySelectorAll('[data-settings-project-id]')
      let idx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) idx = i + 1
      }
      reorderDropRef.current = idx
      setReorderDropIndex(idx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        // Check if dropped on a focus group header → move to that group
        const hoveredGroupId = dragOverGroupRef.current
        if (hoveredGroupId && hoveredGroupId !== '__ungrouped__') {
          await assignProjectToGroup(projectId, hoveredGroupId)
          await fetchProjects()
        } else if (hoveredGroupId === '__ungrouped__') {
          await assignProjectToGroup(projectId, null)
          await fetchProjects()
        } else {
          // Within-zone reorder
          const currentProjects = useProjectsStore.getState().projects
          let list: typeof projects = []
          const z = reorderZoneRef.current
          if (z === 'agents') {
            list = [...currentProjects.filter((p) => p.agentMode === 'agent' || p.agentMode === 'custom')]
          } else if (z === 'pinned') {
            list = [...currentProjects.filter((p) => p.pinned && (!p.agentMode || p.agentMode === 'off'))]
          } else if (z === 'ungrouped' || z === 'flat') {
            list = [...currentProjects.filter((p) => !p.pinned && !p.focusGroupId)]
          } else if (z?.startsWith('group:')) {
            const gid = z.slice(6)
            list = [...currentProjects.filter((p) => p.focusGroupId === gid)]
          }

          const di = reorderDropRef.current
          const fromIdx = list.findIndex((p) => p.id === projectId)
          if (fromIdx >= 0 && di !== null && fromIdx !== di && fromIdx !== di - 1) {
            const item = list.splice(fromIdx, 1)[0]
            const insertAt = di > fromIdx ? di - 1 : di
            list.splice(insertAt, 0, item)
            reorderProjects(list.map((p) => p.id))
          }
        }
      }

      setReorderDragId(null)
      setReorderZone(null)
      setReorderDropIndex(null)
      setDragOverGroupId(null)
      reorderDropRef.current = null
      reorderZoneRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [reorderProjects, assignProjectToGroup, fetchProjects])


  // Build flat list of all visible projects for keyboard navigation
  const allVisibleProjects = useMemo(() => {
    const result: typeof projects = []
    result.push(...agentPinnedProjects)
    result.push(...regularPinnedProjects)
    if (focusGroupsEnabled) {
      for (const group of focusGroups) {
        const gp = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && !agentIds.has(p.id) && matchesSearch(p))
        result.push(...gp)
      }
      result.push(...ungroupedProjects)
    } else {
      const flat = projects.filter((p) => !agentIds.has(p.id) && !p.pinned && matchesSearch(p))
      result.push(...flat)
    }
    return result
  }, [agentPinnedProjects, regularPinnedProjects, focusGroups, focusGroupsEnabled, projects, agentIds, ungroupedProjects, matchesSearch])

  // Reset keyboard index when search changes
  useEffect(() => { setKeyboardIndex(-1) }, [searchQuery])

  // Keyboard navigation in search
  const handleSearchKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, allVisibleProjects.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < allVisibleProjects.length) {
      e.preventDefault()
      setSelectedProjectId(allVisibleProjects[keyboardIndex].id)
    }
  }, [allVisibleProjects, keyboardIndex])

  // Scroll keyboard-selected item into view
  useEffect(() => {
    if (keyboardIndex >= 0 && allVisibleProjects[keyboardIndex]) {
      const el = document.querySelector(`[data-settings-project-id="${allVisibleProjects[keyboardIndex].id}"]`)
      el?.scrollIntoView({ block: 'nearest' })
    }
  }, [keyboardIndex, allVisibleProjects])

  // Right-click context menu for workspace rows
  const handleProjectContextMenu = useCallback(async (e: React.MouseEvent, p: typeof projects[number]) => {
    e.preventDefault()
    e.stopPropagation()

    const menuItems: { id: string; label: string }[] = [
      { id: 'pin', label: p.pinned ? 'Unpin' : 'Pin to top' },
    ]

    // Add "Move to" options if focus groups exist
    if (focusGroupsEnabled && focusGroups.length > 0) {
      menuItems.push({ id: '__separator__', label: '─' })
      for (const group of focusGroups) {
        if (p.focusGroupId === group.id) continue // skip current group
        menuItems.push({ id: `move:${group.id}`, label: `Move to ${group.name}` })
      }
      if (p.focusGroupId) {
        menuItems.push({ id: 'move:__none__', label: 'Remove from group' })
      }
    }

    const clickedId = await showContextMenu(menuItems)
    if (!clickedId) return

    if (clickedId === 'pin') {
      await invoke('projects_update', { id: p.id, pinned: p.pinned ? 0 : 1 })
      await fetchProjects()
    } else if (clickedId.startsWith('move:')) {
      const groupId = clickedId.replace('move:', '')
      await assignProjectToGroup(p.id, groupId === '__none__' ? null : groupId)
      await fetchProjects()
    }
  }, [focusGroupsEnabled, focusGroups, fetchProjects, assignProjectToGroup])

  // Workspace row renderer (called as function, NOT as <Component/>, to avoid unmount/remount flicker)
  const renderProjectRow = (p: typeof projects[number], zone: string, containerSelector: string) => {
    const isSelected = selectedProjectId === p.id
    const isDragged = reorderDragId === p.id
    const kbIdx = allVisibleProjects.findIndex((vp) => vp.id === p.id)
    const isKeyboardHighlighted = kbIdx >= 0 && kbIdx === keyboardIndex
    return (
      <div
        data-settings-project-id={p.id}
        onClick={() => setSelectedProjectId(p.id)}
        onContextMenu={(e) => handleProjectContextMenu(e, p)}
        onMouseDown={(e) => { if (e.button === 0) handleReorderMouseDown(e, p.id, zone, containerSelector) }}
        className={`flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer group select-none ${
          isSelected
            ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
            : isKeyboardHighlighted
              ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
              : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        } ${isDragged ? 'opacity-30' : ''} cursor-grab active:cursor-grabbing`}
      >
        <ProjectAvatar
          projectPath={p.path}
          projectName={p.name}
          projectColor={p.color}
          projectId={p.id}
          iconUrl={p.iconUrl}
          size={20}
        />
        <span className="text-xs truncate flex-1">{p.name}</span>
        <button
          onClick={async (e) => {
            e.stopPropagation()
            const newPinned = p.pinned ? 0 : 1
            await invoke('projects_update', { id: p.id, pinned: newPinned })
            const store = useProjectsStore.getState()
            useProjectsStore.setState({
              projects: store.projects.map((proj) =>
                proj.id === p.id ? { ...proj, pinned: newPinned } : proj
              )
            })
          }}
          className={`flex-shrink-0 p-0.5 transition-colors ${
            p.pinned
              ? 'text-[var(--color-accent)]'
              : 'text-transparent group-hover:text-[var(--color-text-muted)] hover:!text-[var(--color-accent)]'
          }`}
          title={p.pinned ? 'Unpin' : 'Pin to top'}
        >
          <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
            <path d="M9.828.722a.5.5 0 0 1 .354.146l4.95 4.95a.5.5 0 0 1-.707.707l-.71-.71-3.18 3.18a3.5 3.5 0 0 1-.4.3L11 11.106V14.5a.5.5 0 0 1-.854.354L7.5 12.207 4.854 14.854a.5.5 0 0 1-.708-.708L6.793 11.5 4.146 8.854A.5.5 0 0 1 4.5 8h3.394a3.5 3.5 0 0 0 .3-.4l3.18-3.18-.71-.71a.5.5 0 0 1 .354-.854z" />
          </svg>
        </button>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0">
      {/* ── Left panel: focus group toggle + organized workspace list ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col">
        {/* Focus groups toggle at top */}
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)] flex items-center justify-between">
          <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
            Focus Groups
          </span>
          <button
            onClick={() => setFocusGroupsEnabled(!focusGroupsEnabled)}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              focusGroupsEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
                focusGroupsEnabled ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </div>

        {/* Alphabetize buttons */}
        <div className="px-3 py-1.5 flex gap-1.5 border-b border-[var(--color-border)]">
          <button
            onClick={async () => {
              if (focusGroupsEnabled) {
                const sorted = [...focusGroups].sort((a, b) => a.name.localeCompare(b.name))
                await reorderFocusGroups(sorted.map((g) => g.id))
              }
            }}
            className="flex-1 px-1.5 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-white/[0.03] hover:bg-white/[0.06] transition-colors no-drag cursor-pointer"
            title="Sort focus groups A-Z"
          >
            A→Z Groups
          </button>
          <button
            onClick={async () => {
              const sorted = [...projects].sort((a, b) => a.name.localeCompare(b.name))
              await reorderProjects(sorted.map((p) => p.id))
            }}
            className="flex-1 px-1.5 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-white/[0.03] hover:bg-white/[0.06] transition-colors no-drag cursor-pointer"
            title="Sort workspaces A-Z within groups"
          >
            A→Z Workspaces
          </button>
        </div>

        {/* Search bar */}
        <div className="px-2 py-1.5">
          <input
            ref={searchInputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search workspaces..."
            className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
          />
        </div>

        {/* Workspace list — pinned at top, then groups or flat */}
        <div className="flex-1 overflow-y-auto px-1 py-1">
          {/* ── Agent workspaces ── */}
          {agentPinnedProjects.length > 0 && (
            <div className="mb-1 pb-1 border-b border-[var(--color-border)]">
              <div className="px-2 pt-1 pb-1 flex items-center gap-1.5">
                <span className="text-[10px] font-semibold text-[var(--color-accent)] uppercase tracking-wider">
                  Agents
                </span>
                <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-accent)]/10 text-[var(--color-accent)]">
                  {agentPinnedProjects.length}
                </span>
              </div>
              <div data-reorder-zone="agents">
                {agentPinnedProjects.map((p, idx) => (
                  <div key={p.id} className="border-l-2 border-[var(--color-accent)]">
                    {reorderZone === 'agents' && reorderDropIndex === idx && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                    {renderProjectRow(p, 'agents', "[data-reorder-zone='agents']")}
                  </div>
                ))}
                {reorderZone === 'agents' && reorderDropIndex === agentPinnedProjects.length && (
                  <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                )}
              </div>
            </div>
          )}

          {/* ── Pinned workspaces ── */}
          {regularPinnedProjects.length > 0 && (
            <div className="mb-1 pb-1 border-b border-[var(--color-border)]">
              <div className="px-2 pt-1 pb-1">
                <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                  Pinned
                </span>
              </div>
              <div data-reorder-zone="pinned">
                {regularPinnedProjects.map((p, idx) => (
                  <div key={p.id}>
                    {reorderZone === 'pinned' && reorderDropIndex === idx && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                    {renderProjectRow(p, 'pinned', "[data-reorder-zone='pinned']")}
                  </div>
                ))}
                {reorderZone === 'pinned' && reorderDropIndex === regularPinnedProjects.length && (
                  <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                )}
              </div>
            </div>
          )}

          {focusGroupsEnabled ? (
            <>
              {/* Focus group folders */}
              <div data-focus-group-reorder-container>
              {focusGroups.map((group, groupIdx) => {
                const groupProjects = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && !agentIds.has(p.id) && matchesSearch(p))
                const isCollapsed = collapsedGroups.has(group.id)
                const isDragOver = dragOverGroupId === group.id
                const zoneId = `group:${group.id}`
                const isGroupDragged = groupDragId === group.id
                const showGroupDropBefore = groupDropIndex === groupIdx
                const showGroupDropAfter = groupDropIndex === focusGroups.length && groupIdx === focusGroups.length - 1

                // Hide empty focus groups when searching
                if (searchQuery.trim() && groupProjects.length === 0) return null

                return (
                  <div key={group.id} className={`mb-0.5 ${isGroupDragged ? 'opacity-30' : ''}`} data-focus-group-reorder-id={group.id}>
                    {showGroupDropBefore && <div className="h-[2px] bg-[var(--color-accent)] mx-2 mb-0.5" />}
                    {/* Group folder header */}
                    <div
                      data-focus-group-id={group.id}
                      className={`flex items-center gap-1.5 px-2 py-1 cursor-pointer no-drag select-none transition-all duration-150 ${
                        isDragOver
                          ? 'bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)] scale-[1.02]'
                          : 'hover:bg-white/[0.03]'
                      }`}
                      onClick={() => { if (renamingGroupId !== group.id) toggleGroupCollapse(group.id) }}
                      onMouseDown={(e) => handleGroupReorderMouseDown(e, group.id)}
                      onContextMenu={(e) => handleGroupContextMenu(e, group.id)}
                    >
                      {group.color && (
                        <span className="w-1 h-3 flex-shrink-0" style={{ backgroundColor: isDragOver ? 'var(--color-accent)' : group.color }} />
                      )}
                      <svg
                        className={`w-2.5 h-2.5 text-[var(--color-text-muted)] transition-transform flex-shrink-0 ${
                          isCollapsed ? '' : 'rotate-90'
                        }`}
                        fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
                      >
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
                      </svg>
                      {renamingGroupId === group.id ? (
                        <input
                          ref={renameGroupInputRef}
                          type="text"
                          value={renamingGroupName}
                          onChange={(e) => setRenamingGroupName(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleGroupRenameConfirm()
                            else if (e.key === 'Escape') { setRenamingGroupId(null); setRenamingGroupName('') }
                          }}
                          onBlur={handleGroupRenameConfirm}
                          onClick={(e) => e.stopPropagation()}
                          className="text-[11px] font-medium text-[var(--color-text-primary)] flex-1 bg-transparent border-b border-[var(--color-accent)] outline-none px-0 py-0"
                        />
                      ) : (
                        <span className="text-[11px] font-medium text-[var(--color-text-secondary)] flex-1 truncate">
                          {group.name}
                        </span>
                      )}
                      {isDragOver ? (
                        <span className="text-[9px] text-[var(--color-accent)] flex-shrink-0 font-medium">
                          Drop here
                        </span>
                      ) : (
                        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                          {groupProjects.length}
                        </span>
                      )}
                    </div>

                    {!isCollapsed && (
                      <div className="ml-3" data-reorder-zone={zoneId}>
                        {groupProjects.map((p, idx) => (
                          <div key={p.id}>
                            {reorderZone === zoneId && reorderDropIndex === idx && (
                              <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                            )}
                            {renderProjectRow(p, zoneId, `[data-reorder-zone='${zoneId}']`)}
                          </div>
                        ))}
                        {reorderZone === zoneId && reorderDropIndex === groupProjects.length && (
                          <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                        )}
                        {groupProjects.length === 0 && (
                          <div
                            className={`px-2 py-2 text-center text-[10px] text-[var(--color-text-muted)] italic transition-colors ${
                              isDragOver ? 'bg-[var(--color-accent)]/5' : ''
                            }`}
                          >
                            Drop workspaces here
                          </div>
                        )}
                      </div>
                    )}
                    {showGroupDropAfter && <div className="h-[2px] bg-[var(--color-accent)] mx-2 mt-0.5" />}
                  </div>
                )
              })}
              </div>

              {/* Ungrouped workspaces */}
              {ungroupedProjects.length > 0 && (
                <div className="mt-1">
                  <div
                    data-focus-group-id="__ungrouped__"
                    className={`flex items-center gap-1.5 px-2 py-1 text-[11px] font-medium select-none transition-all duration-150 ${
                      dragOverGroupId === '__ungrouped__'
                        ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/15 ring-1 ring-inset ring-[var(--color-accent)] scale-[1.02]'
                        : 'text-[var(--color-text-muted)]'
                    }`}
                  >
                    Ungrouped
                  </div>
                  <div className="ml-1" data-reorder-zone="ungrouped">
                    {ungroupedProjects.map((p, idx) => (
                      <div key={p.id}>
                        {reorderZone === 'ungrouped' && reorderDropIndex === idx && (
                          <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                        )}
                        {renderProjectRow(p, 'ungrouped', "[data-reorder-zone='ungrouped']")}
                      </div>
                    ))}
                    {reorderZone === 'ungrouped' && reorderDropIndex === ungroupedProjects.length && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                  </div>
                </div>
              )}

              {/* Add new group */}
              <div className="mt-2 px-1">
                <div className="flex items-center gap-1">
                  <input
                    type="text"
                    value={newGroupName}
                    onChange={(e) => setNewGroupName(e.target.value)}
                    onKeyDown={(e) => { if (e.key === 'Enter') handleCreateGroup() }}
                    placeholder="+ New group"
                    className="flex-1 px-2 py-1 text-[11px] bg-transparent border border-transparent text-[var(--color-text-muted)] outline-none focus:border-[var(--color-border)] focus:text-[var(--color-text-primary)] no-drag"
                  />
                </div>
              </div>
            </>
          ) : (
            /* Simple flat list when focus groups disabled */
            <div className="space-y-0.5">
              <div className="px-2 pt-1 pb-1">
                <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
                  Workspaces
                </span>
              </div>
              <div data-reorder-zone="flat">
                {projects.filter((p) => !p.pinned && !agentIds.has(p.id)).map((p, idx) => (
                  <div key={p.id}>
                    {reorderZone === 'flat' && reorderDropIndex === idx && (
                      <div className="h-[2px] bg-[var(--color-accent)] mx-2" />
                    )}
                    {renderProjectRow(p, 'flat', "[data-reorder-zone='flat']")}
                  </div>
                ))}
              </div>
              {projects.length === 0 && (
                <div className="px-2 py-6 text-center">
                  <span className="text-xs text-[var(--color-text-muted)]">No workspaces</span>
                </div>
              )}
            </div>
          )}
        </div>

        {/* + New Workspace button */}
        <div className="px-2 py-2 border-t border-[var(--color-border)]">
          <button
            className="w-full flex items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] transition-colors no-drag cursor-pointer"
            onClick={async () => {
              const folderPath = await invoke<string | null>('projects_pick_folder')
              if (folderPath) {
                await useProjectsStore.getState().addProject(folderPath)
                await fetchProjects()
              }
            }}
          >
            <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
            </svg>
            New Workspace
          </button>
        </div>
      </div>

      {/* ── Right panel: selected workspace settings ── */}
      <div className="flex-1 overflow-y-auto p-6 min-h-0 relative">
        {selectedProject ? (
          <ProjectDetail
            project={selectedProject}
            editors={editors}
            focusGroups={focusGroups}
            focusGroupsEnabled={focusGroupsEnabled}
            projectSettings={projectSettings}
            updateProjectSetting={updateProjectSetting}
            removeProject={removeProject}
            assignProjectToGroup={assignProjectToGroup}
            fetchProjects={fetchProjects}
          />
        ) : (
          <div className="flex items-center justify-center h-full">
            <span className="text-xs text-[var(--color-text-muted)]">
              Select a workspace to view its settings
            </span>
          </div>
        )}
      </div>

    </div>
  )
}

// ── Worktree Folders on Disk ─────────────────────────────────────────
function WorktreeFoldersOnDisk({
  project,
  fetchProjects
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [diskWorktrees, setDiskWorktrees] = useState<
    Array<{ path: string; branch: string; isMain: boolean; isBare: boolean }>
  >([])
  const [loading, setLoading] = useState(true)
  const [reopening, setReopening] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    invoke<any[]>('git_worktrees', { path: project.path })
      .then((wts) => {
        if (!cancelled) {
          setDiskWorktrees(wts)
          setLoading(false)
        }
      })
      .catch(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [project.path, project.workspaces])

  // Determine which disk worktrees are active (have a workspace record)
  const activeWorktreePaths = new Set(
    project.workspaces
      .filter((ws) => ws.worktreePath)
      .map((ws) => ws.worktreePath!)
  )
  // Also consider the main project path as active if a branch workspace points to it
  const mainWorkspaceExists = project.workspaces.some((ws) => ws.type === 'branch')

  const handleReopen = async (wt: { path: string; branch: string }): Promise<void> => {
    setReopening(wt.path)
    try {
      await invoke('git_reopen_worktree', {
        projectId: project.id,
        worktreePath: wt.path,
        branch: wt.branch
      })
      await fetchProjects()
    } catch (err) {
      console.error('Reopen worktree failed:', err)
    } finally {
      setReopening(null)
    }
  }

  if (loading) {
    return (
      <div className="space-y-2">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Worktree Folders on Disk
        </h3>
        <p className="text-[10px] text-[var(--color-text-muted)]">Loading...</p>
      </div>
    )
  }

  // Filter out bare worktrees
  const nonBare = diskWorktrees.filter((wt) => !wt.isBare)
  if (nonBare.length === 0) return <></>

  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
        Worktree Folders on Disk
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{nonBare.length}</span>
      </h3>
      <div className="border border-[var(--color-border)]">
        {nonBare.map((wt, i) => {
          const isActive = wt.isMain
            ? mainWorkspaceExists
            : activeWorktreePaths.has(wt.path)
          const isClosed = !isActive

          return (
            <div
              key={wt.path}
              className={`flex items-center gap-2 px-3 py-1.5 ${
                i < nonBare.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <svg
                className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                {wt.isMain ? (
                  <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
                ) : (
                  <path strokeLinecap="round" strokeLinejoin="round" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A2 2 0 013 12V7a4 4 0 014-4z" />
                )}
              </svg>
              <div className="flex-1 min-w-0">
                <span className="text-xs text-[var(--color-text-primary)] truncate block">
                  {wt.branch}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] truncate block" title={wt.path}>
                  {wt.path.length > 50 ? '...' + wt.path.slice(-47) : wt.path}
                </span>
              </div>
              {isActive ? (
                <span className="text-[10px] text-green-400 flex-shrink-0">(active)</span>
              ) : (
                <button
                  onClick={() => handleReopen(wt)}
                  disabled={reopening === wt.path}
                  className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
                >
                  {reopening === wt.path ? 'Reopening...' : 'Reopen'}
                </button>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Workspace Detail (right panel content) ─────────────────────────────
function ProjectDetail({
  project,
  editors,
  focusGroups,
  focusGroupsEnabled,
  projectSettings,
  updateProjectSetting,
  removeProject,
  assignProjectToGroup,
  fetchProjects
}: {
  project: ReturnType<typeof useProjectsStore.getState>['projects'][number]
  editors: string[]
  focusGroups: ReturnType<typeof useFocusGroupsStore.getState>['focusGroups']
  focusGroupsEnabled: boolean
  projectSettings: Record<string, Record<string, any>>
  updateProjectSetting: (projectId: string, key: string, value: string) => void
  removeProject: (id: string) => Promise<void>
  assignProjectToGroup: (projectId: string, groupId: string | null) => Promise<void>
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [iconLoading, setIconLoading] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const [agentEditorOpen, setAgentEditorOpen] = useState(false)
  const [agentEditorName, setAgentEditorName] = useState('')
  // The WakeupEditor lives alongside the other "agent editor"
  // takeovers (ClaudeMdEditor, AgentPersonaEditor, ProjectContextEditor)
  // so it fills the Settings content area without colliding with the
  // workspaces sidebar's stacking context. State is lifted from
  // HeartbeatsPanel so the editor can render at this level.
  const [wakeupEditingHb, setWakeupEditingHb] = useState<HeartbeatRow | null>(null)
  // Heartbeat refresh nonce — incremented when the wakeup editor
  // closes so the panel's k2so_heartbeat_list query re-runs and
  // picks up any edits the agent made to the row.
  const [hbRefreshNonce, setHbRefreshNonce] = useState(0)
  // When agentMode is 'off' and there are no historical fires for this
  // workspace, there's no audit to show — collapse the right column so
  // we don't leave an empty frame next to every Off workspace.
  const [historyEmpty, setHistoryEmpty] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  // Close editor when project changes (user navigated away without using back button)
  useEffect(() => {
    setAgentEditorOpen(false)
    setAgentEditorName('')
    setWakeupEditingHb(null)
  }, [project.id])


  const handleDetectIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await invoke('projects_detect_icon', { projectId: project.id })
      await fetchProjects()
    } catch (err) {
      console.error('Icon detection failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleUploadClick = (): void => {
    fileInputRef.current?.click()
  }

  const handleFileSelected = (e: React.ChangeEvent<HTMLInputElement>): void => {
    const file = e.target.files?.[0]
    if (!file) return

    const reader = new FileReader()
    reader.onload = () => {
      setCropImage(reader.result as string)
    }
    reader.readAsDataURL(file)

    // Reset input so the same file can be re-selected
    e.target.value = ''
  }

  const handleCropConfirm = async (croppedDataUrl: string): Promise<void> => {
    setCropImage(null)
    setIconLoading(true)
    try {
      await invoke('projects_update', { id: project.id, iconUrl: croppedDataUrl })
      await fetchProjects()
    } catch (err) {
      console.error('Icon save failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleClearIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await invoke('projects_clear_icon', { projectId: project.id })
      await fetchProjects()
    } catch (err) {
      console.error('Icon clear failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const firstLetter = project.name.charAt(0).toUpperCase()

  // Full-screen agent editor takeover (same pattern as CustomThemeCreator)
  if (agentEditorOpen && agentEditorName) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          {agentEditorName === '__project_context__' ? (
            <ProjectContextEditor
              projectPath={project.path}
              projectName={project.name}
              onClose={() => setAgentEditorOpen(false)}
            />
          ) : agentEditorName === '__claude_md__' ? (
            <ClaudeMdEditor
              projectPath={project.path}
              projectName={project.name}
              onClose={() => setAgentEditorOpen(false)}
            />
          ) : (
            <AgentPersonaEditor
              agentName={agentEditorName}
              projectPath={project.path}
              onClose={() => setAgentEditorOpen(false)}
            />
          )}
        </div>
      </SectionErrorBoundary>
    )
  }

  // Heartbeat WAKEUP.md takeover — same pattern as ClaudeMdEditor.
  // The state lives at this level (not in HeartbeatsPanel) so the
  // editor fills the Settings content area cleanly instead of being
  // squeezed inside the right-rail aside or fighting the workspaces
  // sidebar's stacking context with a fixed overlay.
  if (wakeupEditingHb) {
    const mode = (project.agentMode || 'off') as string
    const wakeupAgentName =
      mode === 'manager' || mode === 'coordinator' || mode === 'pod'
        ? '__lead__'
        : mode === 'agent'
          ? 'k2so-agent'
          : project.name.toLowerCase().replace(/\s+/g, '-')
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <WakeupEditor
            projectPath={project.path}
            agentName={wakeupAgentName}
            heartbeat={wakeupEditingHb}
            otherHeartbeats={[]}
            onClose={() => {
              setWakeupEditingHb(null)
              setHbRefreshNonce((n) => n + 1)
            }}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  return (
    <>
    {cropImage && (
      <IconCropDialog
        imageDataUrl={cropImage}
        onConfirm={handleCropConfirm}
        onCancel={() => setCropImage(null)}
      />
    )}
    <div className={`grid gap-8 ${
      (project.agentMode || 'off') === 'off' && historyEmpty
        ? 'grid-cols-[minmax(0,42rem)]'
        : 'grid-cols-[minmax(0,42rem)_minmax(0,1fr)]'
    }`}>
    <div className="space-y-6 min-w-0">
      {/* ── Header ── */}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-base font-medium text-[var(--color-text-primary)]">{project.name}</h2>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-1 break-all">{project.path}</p>
        </div>
        <button
          onClick={() => {
            const defaultWs = project.workspaces?.[0]
            if (defaultWs) {
              useProjectsStore.getState().setActiveWorkspace(project.id, defaultWs.id)
            }
            useSettingsStore.getState().closeSettings()
          }}
          className="flex-shrink-0 px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
        >
          Open Workspace
        </button>
      </div>

      {/* ── Group 1: Workspace — Icon, Color, Focus Group ── */}
      <SettingsGroup title="Workspace">
        {/* Icon */}
        <div className="flex items-center gap-4 py-2">
          <div
            className="flex-shrink-0 flex items-center justify-center overflow-hidden"
            style={{
              width: 48,
              height: 48,
              backgroundColor: project.iconUrl ? 'transparent' : project.color,
              border: project.iconUrl ? `2px solid ${project.color}` : 'none'
            }}
          >
            {project.iconUrl ? (
              <img
                src={project.iconUrl}
                alt={project.name}
                style={{ width: '100%', height: '100%', objectFit: 'cover', objectPosition: 'center', display: 'block' }}
              />
            ) : (
              <span
                className="text-white font-bold"
                style={{ fontSize: 22, lineHeight: 1 }}
              >
                {firstLetter}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleDetectIcon}
              disabled={iconLoading}
              className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              {iconLoading ? 'Working...' : 'Detect'}
            </button>
            <button
              onClick={handleUploadClick}
              disabled={iconLoading}
              className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              Upload
            </button>
            <input
              ref={fileInputRef}
              type="file"
              accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
              className="hidden"
              onChange={handleFileSelected}
            />
            {project.iconUrl && (
              <button
                onClick={handleClearIcon}
                disabled={iconLoading}
                className="px-2.5 py-1 text-xs text-red-400 border border-red-500/30 hover:bg-red-500/10 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
              >
                Remove
              </button>
            )}
          </div>
        </div>

        {/* Color */}
        <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Color</span>
          <div className="flex items-center gap-1.5">
            {['#3b82f6', '#ef4444', '#22c55e', '#f59e0b', '#a855f7', '#ec4899', '#06b6d4', '#64748b'].map((color) => (
              <button
                key={color}
                onClick={async () => {
                  await invoke('projects_update', { id: project.id, color })
                  await fetchProjects()
                }}
                className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform ${
                  project.color === color ? 'scale-125 ring-1 ring-white/50' : 'hover:scale-110'
                }`}
                style={{ backgroundColor: color }}
              />
            ))}
          </div>
        </div>

        {/* Focus Group */}
        {focusGroupsEnabled && (
          <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-secondary)]">Focus Group</span>
            <SettingDropdown
              value={project.focusGroupId ?? ''}
              options={[
                { value: '', label: 'No Group' },
                ...focusGroups.map((g) => ({ value: g.id, label: g.name })),
              ]}
              onChange={async (v) => {
                await assignProjectToGroup(project.id, v || null)
                await fetchProjects()
              }}
            />
          </div>
        )}

        {/* Workspace Knowledge — canonical SKILL file all CLI harnesses read. */}
        {/* Lives at .k2so/skills/k2so/SKILL.md and is symlinked to Claude, */}
        {/* OpenCode, Pi, plus marker-injected into AGENTS.md + Copilot paths. */}
        {/* One edit here propagates to every CLI LLM in the workspace. */}
        <div className="pt-3 border-t border-[var(--color-border)]">
          <div className="flex items-center justify-between">
            <div>
              <span className="text-xs text-[var(--color-text-secondary)]">Workspace Knowledge</span>
              <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
                Canonical <span className="font-mono">SKILL.md</span> every CLI LLM reads — symlinked to Claude, OpenCode, Pi, and marker-injected into AGENTS.md + Copilot paths.
              </p>
            </div>
            <button
              onClick={() => { setAgentEditorName('__claude_md__'); setAgentEditorOpen(true) }}
              className="px-5 py-1.5 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer flex-shrink-0 whitespace-nowrap"
            >
              Manage Knowledge
            </button>
          </div>
        </div>
      </SettingsGroup>

      {/* ── Group 2: Agent Settings — Mode tabs, Heartbeat, Agents list ── */}
      {useSettingsStore.getState().agenticSystemsEnabled && <SettingsGroup
        title="Agent Settings"
        badge={
          <span
            className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
            title="Agentic systems are in beta — interface and behavior may change"
          >
            beta
          </span>
        }
      >
        <div className="space-y-2">
          {/* Mode selector */}
          <div className="flex gap-1">
            {(['off', 'agent', 'manager', 'custom'] as const).map((mode) => {
              const isActive = (project.agentMode || 'off') === mode || (mode === 'manager' && (project.agentMode === 'coordinator' || project.agentMode === 'pod'))
              const labels = { off: 'Off', custom: 'Custom Agent', agent: 'K2SO Agent', manager: 'Workspace Manager' }
              return (
                <button
                  key={mode}
                  onClick={async () => {
                    const currentMode = project.agentMode || 'off'
                    if (currentMode === mode) return

                    // Confirm before modifying CLAUDE.md — explain what will happen
                    const fromLabel = currentMode === 'off' ? null : labels[currentMode as keyof typeof labels]
                    const toLabel = labels[mode]

                    if (mode === 'off') {
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Disable ${fromLabel} Mode`,
                        message: [
                          'This will:',
                          '',
                          '• Move CLAUDE.md to .k2so/CLAUDE.md.disabled',
                          '• Your content is preserved and restored if you re-enable',
                          '• The heartbeat will be turned off if active',
                        ].join('\n'),
                        confirmLabel: 'Disable',
                      })
                      if (!confirmed) return
                    } else if (mode === 'custom') {
                      const lines = [
                        'Train a single agent to operate any software via the heartbeat.',
                        '',
                        'What happens:',
                        '• No CLAUDE.md is generated — the agent runs from its persona only',
                        '• Use "Manage Persona" to define its behavior with the AI editor',
                        '• Worktrees are disabled in this mode',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    } else if (mode === 'agent') {
                      const lines = [
                        'A K2SO planner agent that helps you build PRDs, milestones, and technical plans.',
                        '',
                        'What happens:',
                        '• Generates a CLAUDE.md with K2SO planner instructions',
                        '• If a user-written CLAUDE.md exists, it won\'t be overwritten',
                        '  (the generated version is saved to .k2so/CLAUDE.md.generated)',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    } else if (mode === 'manager') {
                      const lines = [
                        'A workspace manager delegates work to agent templates that execute in parallel worktrees.',
                        '',
                        'What happens:',
                        '• Generates a CLAUDE.md with manager instructions',
                        '• A manager agent is created automatically',
                        '• If a user-written CLAUDE.md exists, it won\'t be overwritten',
                        '  (the generated version is saved to .k2so/CLAUDE.md.generated)',
                      ]
                      if (currentMode !== 'off') {
                        lines.push('', `Switching from ${fromLabel}:`, '• The current CLAUDE.md will be moved to .k2so/CLAUDE.md.disabled')
                      }
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: `Enable ${toLabel} Mode`,
                        message: lines.join('\n'),
                        confirmLabel: `Enable ${toLabel} Mode`,
                      })
                      if (!confirmed) return
                    }

                    if (currentMode !== 'off') {
                      await invoke('k2so_agents_disable_workspace_claude_md', {
                        projectPath: project.path,
                      }).catch(console.error)
                    }

                    await invoke('projects_update', { id: project.id, agentMode: mode })

                    if (mode === 'agent' || mode === 'manager') {
                      await invoke('k2so_agents_regenerate_workspace_skill', {
                        projectPath: project.path,
                      }).catch(console.error)
                    }

                    if (mode === 'off' && project.heartbeatEnabled) {
                      await invoke('projects_update', { id: project.id, heartbeatEnabled: 0 })
                    }

                    await fetchProjects()
                  }}
                  className={`flex-1 px-2 py-1.5 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                    isActive
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                  }`}
                >
                  {labels[mode]}
                </button>
              )
            })}
          </div>

          <p className="text-[10px] text-[var(--color-text-muted)]">
            {(project.agentMode || 'off') === 'off' && 'No agent features enabled for this workspace.'}
            {(project.agentMode || 'off') === 'custom' && 'Custom Agent — train agents to operate any software via the heartbeat. Customize each agent\'s behavior with the AI persona editor.'}
            {(project.agentMode || 'off') === 'agent' && 'K2SO Agent — a planner that helps you build PRDs, milestones, and technical plans for this workspace.'}
            {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || project.agentMode === 'pod') && 'Workspace Manager — delegates work to agent templates that execute in parallel worktrees.'}
          </p>

          {/* Agent identity — Persona + name FIRST (above State) so the
              reading order is identity → lifecycle-state → schedule. */}
          {(project.agentMode || 'off') === 'custom' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <CustomAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}
          {(project.agentMode || 'off') === 'agent' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <K2SOAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* State selector — only when a mode is active */}
          {(project.agentMode || 'off') !== 'off' && (
            <StateSelector projectId={project.id} currentStateId={project.stateId} />
          )}

          {/* Heartbeats moved to the right column (next to History +
              Context Layers) so the wake-related surfaces live together
              and the main settings column stays focused on workspace
              identity/mode/worktree setup. See the aside below. */}

          {/* Agent templates list — only in Manager mode */}
          {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || project.agentMode === 'pod') && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <ProjectAgentsPanel projectPath={project.path} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* Connected Workspaces — for manager, coordinator, and custom modes */}
          {((project.agentMode || 'off') === 'manager' || project.agentMode === 'coordinator' || (project.agentMode || 'off') === 'custom') && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <ConnectedWorkspacesPanel projectId={project.id} />
            </div>
          )}
        </div>
      </SettingsGroup>}

      {/* ── Group 3: Worktree Management ── */}
      <SettingsGroup title="Worktrees">
        {/* Worktrees table */}
        <div className={project.workspaces.length > 0 ? '' : 'hidden'}>
          <div className="border border-[var(--color-border)]">
            {project.workspaces.map((ws, i) => (
              <div
                key={ws.id}
                className={`flex items-center gap-2 px-3 py-1.5 ${
                  i < project.workspaces.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                }`}
              >
                <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-muted)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
                </svg>
                <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">{ws.name}</span>
                {ws.branch && (
                  <span className="text-[10px] text-[var(--color-text-muted)] truncate max-w-[120px]">{ws.branch}</span>
                )}
                <span className="text-[10px] text-[var(--color-text-muted)]">{ws.type}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Worktree Folders on Disk */}
        <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />
      </SettingsGroup>

      {/* ── Group 4: Chat Migrations ── */}
      <SettingsGroup title="Chat Migrations">
        <CursorMigrationPanel projectPath={project.path} />
      </SettingsGroup>

      {/* ── Danger zone ── */}
      <div className="pt-4 border-t border-[var(--color-border)]">
        <button
          onClick={() => removeProject(project.id)}
          className="px-3 py-1 text-xs text-red-400 border border-red-500/30 hover:bg-red-500/10 no-drag cursor-pointer"
        >
          Remove Workspace
        </button>
      </div>
    </div>

    {/* Right column — wake-related surfaces grouped together: Heartbeats
        (the schedule), Context Layers (what ships in each wake), and the
        fire History (what actually happened). Sticks to the top while
        the left column scrolls so the wake picture stays visible next to
        whatever setting you're editing. Hidden entirely when the
        workspace is Off AND has no historical fire rows, so an Off
        workspace that was never an agent doesn't get a dead frame. */}
    {!((project.agentMode || 'off') === 'off' && historyEmpty) && (
      <aside className="min-w-0 sticky top-0 self-start space-y-4">
        {(project.agentMode || 'off') !== 'off' && (
          <>
            <HeartbeatsPanel
              key={`hb-${hbRefreshNonce}`}
              projectPath={project.path}
              agentMode={project.agentMode || null}
              agentName={(() => {
                const mode = project.agentMode || 'off'
                if (mode === 'manager' || mode === 'coordinator' || mode === 'pod') return '__lead__'
                if (mode === 'agent') return 'k2so-agent'
                // Custom mode — backend find_primary_agent scans for the
                // custom-typed agent by reading AGENT.md frontmatter. The
                // UI just needs a display name; use the project.name as a
                // reasonable fallback until we wire a lookup.
                return project.name.toLowerCase().replace(/\s+/g, '-')
              })()}
              onConfigureWakeup={(row) => setWakeupEditingHb(row)}
            />
            <ShowHeartbeatSessionsToggle projectPath={project.path} />
          </>
        )}
        {(project.agentMode || 'off') !== 'off' && (
          <ContextLayersPreview
            projectPath={project.path}
            agentMode={project.agentMode || null}
            onOpenSettings={() => {
              // Deep-link to Settings → Agent Skills. The layer stack is
              // read-only; edits go through the Agent Skills section.
              useSettingsStore.getState().setSection('agent-skills')
            }}
          />
        )}
        <HistoryPanel projectPath={project.path} onEmptyChange={setHistoryEmpty} />
      </aside>
    )}
    </div>
    </>
  )
}

// ── Show Heartbeat Sessions Toggle ──────────────────────────────────
//
// Per-workspace flag controlling whether heartbeat fires open a tab in
// the Tauri window. Default OFF (silent autonomous run, the v2-headless
// vision default). When ON, each fire opens a background tab (no focus
// steal); the user closes it when done auditing. State lives in
// `projects.show_heartbeat_sessions` (migration 0034).

function ShowHeartbeatSessionsToggle({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [enabled, setEnabled] = useState<boolean | null>(null) // null = loading
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    invoke<boolean>('k2so_workspace_get_show_heartbeat_sessions', { projectPath })
      .then((v) => { if (!cancelled) setEnabled(v) })
      .catch((err) => {
        // Surface the error rather than silently defaulting — a missing
        // row here means the project_id resolution broke and the toggle
        // would otherwise lie to the user about its state.
        console.error('[show-heartbeat-sessions] read failed', err)
      })
    return () => { cancelled = true }
  }, [projectPath])

  const toggle = async (): Promise<void> => {
    if (enabled === null || busy) return
    const next = !enabled
    setBusy(true)
    setEnabled(next) // optimistic
    try {
      await invoke('k2so_workspace_set_show_heartbeat_sessions', {
        projectPath,
        enabled: next,
      })
    } catch (err) {
      console.error('[show-heartbeat-sessions] write failed', err)
      setEnabled(!next) // revert on failure
    } finally {
      setBusy(false)
    }
  }

  if (enabled === null) {
    return (
      <div className="text-[10px] text-[var(--color-text-muted)]">Loading…</div>
    )
  }

  return (
    <div className="border border-[var(--color-border)] p-3">
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          disabled={busy}
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={enabled ? 'Heartbeat fires open background tabs' : 'Heartbeat fires run silently'}
        >
          <span
            className={`w-2.5 h-2.5 bg-white block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Show heartbeat sessions in tabs
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
            {enabled
              ? 'Each heartbeat fire opens a background tab in this window. Tabs persist until you close them. Audit the agent\'s work as it happens.'
              : 'Heartbeat fires run silently in the daemon (recommended). Audit them on demand from the sidebar Heartbeats panel.'}
          </div>
        </div>
      </div>
    </div>
  )
}

// ── K2SO Agents Panel ───────────────────────────────────────────────

interface K2soAgentInfo {
  name: string
  role: string
  inboxCount: number
  activeCount: number
  doneCount: number
  isCoordinator: boolean // legacy field name from backend; true = manager agent
}

function AgentKebabMenu({ onSettings, onDelete }: { onSettings: () => void; onDelete?: () => void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handleClick)
    return () => document.removeEventListener('mousedown', handleClick)
  }, [open])

  return (
    <div className="relative" ref={menuRef}>
      <button
        onClick={() => setOpen(!open)}
        className="px-1 py-0.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
        title="More options"
      >
        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
          <circle cx="8" cy="3" r="1.5" />
          <circle cx="8" cy="8" r="1.5" />
          <circle cx="8" cy="13" r="1.5" />
        </svg>
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-lg min-w-[140px]">
          <button
            onClick={() => { setOpen(false); onSettings() }}
            className="w-full text-left px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
          >
            Settings
          </button>
          {onDelete && (
            <button
              onClick={() => { setOpen(false); onDelete() }}
              className="w-full text-left px-3 py-1.5 text-[11px] text-red-400 hover:bg-red-500/10 hover:text-red-300 transition-colors no-drag cursor-pointer"
            >
              Delete Agent
            </button>
          )}
        </div>
      )}
    </div>
  )
}

// ── Adaptive Heartbeat Config (Phase indicator, interval, force-wake) ──

interface HeartbeatConfig {
  mode: string
  intervalSeconds: number
  phase: string
  maxIntervalSeconds: number
  minIntervalSeconds: number
  costBudget: string
  consecutiveNoOps: number
  autoBackoff: boolean
  lastWake: string | null
  nextWake: string | null
}

const PHASE_COLORS: Record<string, { dot: string; label: string }> = {
  setup: { dot: 'bg-blue-400', label: 'text-blue-400' },
  active: { dot: 'bg-green-400', label: 'text-green-400' },
  monitoring: { dot: 'bg-amber-400', label: 'text-amber-400' },
  idle: { dot: 'bg-gray-400', label: 'text-gray-400' },
  blocked: { dot: 'bg-red-400', label: 'text-red-400' },
}

function AdaptiveHeartbeatConfig({ projectPath }: { projectPath: string }): React.JSX.Element {
  const [config, setConfig] = useState<HeartbeatConfig | null>(null)
  const [agents, setAgents] = useState<{ name: string; type: string }[]>([])
  const [selectedAgent, setSelectedAgent] = useState<string>('')

  // Load custom agents for this project
  useEffect(() => {
    invoke<{ name: string; agentType: string }[]>('k2so_agents_list', { projectPath })
      .then((list) => {
        const customAgents = list.filter((a) => a.agentType === 'custom')
        setAgents(customAgents.map((a) => ({ name: a.name, type: a.agentType })))
        if (customAgents.length > 0 && !selectedAgent) {
          setSelectedAgent(customAgents[0].name)
        }
      })
      .catch(() => {})
  }, [projectPath])

  // Load heartbeat config for selected agent
  useEffect(() => {
    if (!selectedAgent) return
    invoke<HeartbeatConfig>('k2so_agents_get_heartbeat', { projectPath, agentName: selectedAgent })
      .then(setConfig)
      .catch(() => setConfig(null))
  }, [projectPath, selectedAgent])

  if (!config || agents.length === 0) return <></>

  const phaseStyle = PHASE_COLORS[config.phase] || PHASE_COLORS.monitoring
  const formatInterval = (s: number) => s >= 3600 ? `${Math.round(s / 3600)}h` : s >= 60 ? `${Math.round(s / 60)}m` : `${s}s`

  const handleUpdate = async (updates: { interval?: number; phase?: string }) => {
    try {
      const result = await invoke<HeartbeatConfig>('k2so_agents_set_heartbeat', {
        projectPath,
        agentName: selectedAgent,
        interval: updates.interval ?? null,
        phase: updates.phase ?? null,
        mode: null,
        costBudget: null,
        forceWake: null,
      })
      setConfig(result)
    } catch (err) {
      console.error('[heartbeat] Update failed:', err)
    }
  }

  const handleForceWake = async () => {
    try {
      // Set next_wake to now so the scheduler picks it up immediately
      const result = await invoke<HeartbeatConfig>('k2so_agents_set_heartbeat', {
        projectPath,
        agentName: selectedAgent,
        interval: null,
        phase: null,
        mode: null,
        costBudget: null,
        forceWake: true,
      })
      setConfig(result)
      // Trigger immediate triage
      await invoke('k2so_agents_scheduler_tick', { projectPath })
    } catch (err) {
      console.error('[heartbeat] Force wake failed:', err)
    }
  }

  return (
    <div className="py-2 border-t border-[var(--color-border)]">
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2">
          {/* Phase indicator dot */}
          <span className={`w-2 h-2 rounded-full ${phaseStyle.dot}`} />
          <span className={`text-[10px] font-medium ${phaseStyle.label}`}>
            {config.phase}
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)]">
            every {formatInterval(config.intervalSeconds)}
          </span>
          {config.consecutiveNoOps > 0 && (
            <span className="text-[9px] text-[var(--color-text-muted)] opacity-60">
              ({config.consecutiveNoOps} idle)
            </span>
          )}
        </div>
        <button
          onClick={handleForceWake}
          className="px-2 py-0.5 text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag"
        >
          Force Wake
        </button>
      </div>

      {/* Interval presets */}
      <div className="flex gap-1 mb-1.5">
        {[
          { label: '1m', seconds: 60, phase: 'active' },
          { label: '5m', seconds: 300, phase: 'monitoring' },
          { label: '15m', seconds: 900, phase: 'monitoring' },
          { label: '1h', seconds: 3600, phase: 'idle' },
        ].map((preset) => (
          <button
            key={preset.label}
            onClick={() => handleUpdate({ interval: preset.seconds, phase: preset.phase })}
            className={`px-2 py-0.5 text-[9px] border transition-colors cursor-pointer no-drag ${
              config.intervalSeconds === preset.seconds
                ? 'border-[var(--color-accent)] text-[var(--color-accent)] bg-[var(--color-accent)]/10'
                : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {preset.label}
          </button>
        ))}
      </div>

      {/* Last/next wake info */}
      <div className="flex gap-3 text-[9px] text-[var(--color-text-muted)]">
        {config.lastWake && (
          <span>Last: {new Date(config.lastWake).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        )}
        {config.nextWake && (
          <span>Next: {new Date(config.nextWake).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        )}
        {config.autoBackoff && config.consecutiveNoOps >= 3 && (
          <span className="text-amber-400/80">auto-backoff active</span>
        )}
      </div>
    </div>
  )
}

// ── State Selector (per-workspace dropdown) ──────────────────────────────

function StateSelector({ projectId, currentStateId }: { projectId: string; currentStateId?: string | null }): React.JSX.Element {
  const [states, setStates] = useState<StateData[]>([])
  const [selectedId, setSelectedId] = useState(currentStateId || '')

  useEffect(() => {
    invoke<StateData[]>('states_list').then(setStates).catch(() => {})
  }, [])

  useEffect(() => {
    setSelectedId(currentStateId || '')
  }, [currentStateId])

  const handleChange = async (stateId: string) => {
    setSelectedId(stateId)
    try {
      await invoke('projects_update', { id: projectId, stateId: stateId || '' })
      const store = useProjectsStore.getState()
      const updated = store.projects.map((p) =>
        p.id === projectId ? { ...p, stateId: stateId || null } : p
      )
      useProjectsStore.setState({ projects: updated })
    } catch (err) {
      console.error('[state-selector] Update failed:', err)
    }
  }

  if (states.length === 0) return <></>

  const activeState = states.find((t) => t.id === selectedId)

  return (
    <div className="pt-3 pb-1 border-t border-[var(--color-border)]">
      <div className="flex items-center justify-between">
        <span className="text-xs text-[var(--color-text-primary)]">State</span>
        <SettingDropdown
          value={selectedId || ''}
          options={[
            { value: '', label: 'No state' },
            ...states.map((t) => ({ value: t.id, label: t.name })),
          ]}
          onChange={handleChange}
        />
      </div>
      {activeState?.description && (
        <p className="text-[10px] text-[var(--color-text-muted)] mt-1.5 leading-relaxed">{activeState.description}</p>
      )}
      {activeState && (
        <div className="flex flex-wrap gap-x-1.5 gap-y-1 mt-2">
          {CAPABILITIES.map((cap) => {
            const val = activeState[cap.key] as string
            return (
              <span
                key={cap.key}
                className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-[9px] border border-[var(--color-border)] bg-[var(--color-bg)]`}
              >
                <span className="text-[var(--color-text-muted)]">{cap.label}</span>
                <span className={CAP_COLORS[val] || 'text-[var(--color-text-muted)]'}>{CAP_LABELS[val]}</span>
              </span>
            )
          })}
        </div>
      )}
    </div>
  )
}

// ── Project Context Editor (AIFileEditor for .k2so/PROJECT.md) ──────

function ProjectContextEditor({ projectPath, projectName, onClose }: { projectPath: string; projectName: string; onClose: () => void }): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  const filePath = `${projectPath}/.k2so/PROJECT.md`
  const watchDir = `${projectPath}/.k2so`

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Load content
  useEffect(() => {
    invoke<{ content: string }>('fs_read_file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleFileChange = useCallback((c: string) => setContent(c), [])

  const systemPrompt = useMemo(() => [
    `You're helping the user define shared project context for their AI agent workspace.`,
    ``,
    `Project: "${projectName}"`,
    `File: .k2so/PROJECT.md`,
    ``,
    `This file is injected into EVERY agent's CLAUDE.md at launch.`,
    `It should contain project-wide knowledge that all agents need:`,
    ``,
    `• About This Project — what the codebase does, what problem it solves`,
    `• Tech Stack — languages, frameworks, databases, infrastructure`,
    `• Key Directories — important paths and what lives in them`,
    `• Conventions — code style, commit format, PR process, branch naming`,
    `• External Systems — issue trackers, CI dashboards, staging environments`,
    ``,
    `Edit PROJECT.md in the current directory. The user sees a live preview on the right.`,
    ``,
    `Current contents:`,
    content,
  ].join('\n'), [projectName, content])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    if (agentCommand.command === 'claude') {
      return [
        ...baseArgs,
        '--append-system-prompt', systemPrompt,
        `Open and read PROJECT.md in the current directory. This defines shared context for all agents in "${projectName}". Start by asking about their tech stack and project structure.`,
      ]
    }
    return baseArgs
  }, [agentCommand, systemPrompt, projectName])

  return (
    <AIFileEditor
      filePath={filePath}
      watchDir={watchDir}
      cwd={watchDir}
      command={terminalCommand}
      args={terminalArgs}
      title={`Project Context: ${projectName}`}
      instructions={`Editing .k2so/PROJECT.md — shared context injected into all agents at launch.`}
      warningText="Changes here affect all agents in this workspace."
      onFileChange={handleFileChange}
      onClose={onClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">PROJECT.md</span>
              <span className="mx-2">&middot;</span>
              <span>Shared agent context</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex gap-0.5">
                {(['preview', 'edit'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setPreviewMode(mode)}
                    className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                      previewMode === mode
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {mode === 'preview' ? 'Preview' : 'Edit'}
                  </button>
                ))}
              </div>
            </div>
          </div>
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <Markdown remarkPlugins={[remarkGfm]}>
                  {content || '*No content yet. Use the AI assistant to set up your project context.*'}
                </Markdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={content}
                filePath={filePath}
                onSave={async (c) => {
                  try { await invoke('fs_write_file', { path: filePath, content: c }) } catch {}
                }}
                onChange={(c) => setContent(c)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}

// ── Workspace Wake-up Editor — REMOVED in 0.32.6 ──────
// `.k2so/WAKEUP.md` retired; manager wake content now lives in the
// per-row `triage` heartbeat's WAKEUP.md (see Heartbeats panel).

// ── Workspace Knowledge editor (PROJECT.md — the source) ─────────────
// PROJECT.md is the single source for workspace knowledge. K2SO's
// regen pipeline compiles it (plus each agent's AGENT.md) into the
// per-agent SKILL.md files, then symlinks/marker-injects those into
// every CLI harness file (Claude/OpenCode/Pi/Codex/Gemini/Cursor/etc.).
// Editing PROJECT.md is the right surface; editing the compiled
// SKILL.md (the prior shape of this editor) led to silent overwrites
// because the regen pipeline owns the output.

function ClaudeMdEditor({ projectPath, projectName, onClose }: { projectPath: string; projectName: string; onClose: () => void }): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  const filePath = `${projectPath}/.k2so/PROJECT.md`
  const watchDir = `${projectPath}/.k2so`

  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  useEffect(() => {
    invoke<{ content: string }>('fs_read_file', { path: filePath })
      .then((r) => setContent(r.content))
      .catch(() => setContent(''))
  }, [filePath])

  const handleFileChange = useCallback((c: string) => setContent(c), [])

  // Trigger workspace SKILL.md regen on close so PROJECT.md edits
  // propagate to every harness file before the user closes the editor.
  const handleClose = useCallback(async () => {
    try {
      await invoke('k2so_agents_regenerate_workspace_skill', { projectPath })
    } catch (err) {
      console.warn('[workspace-knowledge] regen on close failed:', err)
    }
    onClose()
  }, [projectPath, onClose])

  const systemPrompt = useMemo(() => [
    `You're helping the user edit the workspace knowledge for "${projectName}".`,
    ``,
    `File: .k2so/PROJECT.md (source)`,
    `Path: ${filePath}`,
    ``,
    `This is the SOURCE file. K2SO compiles it (plus each agent's AGENT.md)`,
    `into per-agent SKILL.md files, then propagates that content into every`,
    `CLI harness file (CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules/k2so.mdc,`,
    `.goosehints, .opencode/agent/k2so.md, .pi/skills/k2so/SKILL.md, etc.).`,
    `Edit here once; the regen pipeline updates everywhere on save.`,
    ``,
    `Good content for this file:`,
    `• Project overview — what this codebase does`,
    `• Tech stack — languages, frameworks, key dependencies`,
    `• Key directories — important paths and what lives in them`,
    `• Conventions — code style, commit format, branch naming, PR process`,
    `• Build & test — how to build, run tests, deploy`,
    `• Important notes — gotchas, known issues, things to watch out for`,
    ``,
    `Do NOT include agent-specific role/persona content — that lives in each`,
    `agent's AGENT.md (one per agent under .k2so/agents/<name>/AGENT.md) and`,
    `the user can edit it via Settings → Workspaces → "Manage Persona".`,
    ``,
    `Current contents:`,
    content,
  ].join('\n'), [projectName, filePath, content])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    if (agentCommand.command === 'claude') {
      return [
        ...baseArgs,
        '--append-system-prompt', systemPrompt,
        `Read ${filePath}. Help the user define their workspace knowledge for "${projectName}". Start by asking about their tech stack and project structure.`,
      ]
    }
    return baseArgs
  }, [agentCommand, systemPrompt, projectName, filePath])

  return (
    <AIFileEditor
      filePath={filePath}
      watchDir={watchDir}
      cwd={projectPath}
      command={terminalCommand}
      args={terminalArgs}
      title={`Workspace Knowledge: ${projectName}`}
      instructions="Editing .k2so/PROJECT.md — the source for workspace knowledge. K2SO compiles this into every agent's SKILL.md and propagates to CLAUDE.md, AGENTS.md, GEMINI.md, .cursor/rules, .goosehints, etc. Regen runs automatically when you close this editor."
      warningText="This is the source file for the workspace's shared knowledge. Edits compile into every CLI LLM harness on save."
      onFileChange={handleFileChange}
      onClose={handleClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">PROJECT.md</span>
              <span className="mx-2">&middot;</span>
              <span>Source — compiled into every agent's SKILL.md</span>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                  >
                    +
                  </button>
                </div>
              )}
              <div className="flex gap-0.5">
                {(['preview', 'edit'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setPreviewMode(mode)}
                    className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                      previewMode === mode
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {mode === 'preview' ? 'Preview' : 'Edit'}
                  </button>
                ))}
              </div>
            </div>
          </div>
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <Markdown remarkPlugins={[remarkGfm]}>
                  {content || '*No SKILL.md yet. Use the AI assistant to set up your workspace knowledge, or click Edit to write it manually.*'}
                </Markdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={content}
                filePath={filePath}
                onSave={async (c) => {
                  try { await invoke('fs_write_file', { path: filePath, content: c }) } catch {}
                }}
                onChange={(c) => setContent(c)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}

// ── Custom Agent Persona Button ──────────────────────────────────────

function CustomAgentPersonaButton({ projectPath, projectName, onOpenEditor }: { projectPath: string; projectName: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const derived = projectName.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
  const [ready, setReady] = useState(false)
  const [agentExists, setAgentExists] = useState(false)
  const [agentName, setAgentName] = useState(derived)
  const [nameDraft, setNameDraft] = useState(derived)
  const [nameError, setNameError] = useState<string | null>(null)

  // Load existing Custom agent name if one is already set up for this
  // workspace. First-time render (no agent yet) leaves the draft at the
  // workspace-derived default, ready for the user to edit before creation.
  useEffect(() => {
    const init = async () => {
      try {
        const agents = await invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
        const existing = agents.find((a: any) => a.agentType === 'custom')
        if (existing) {
          setAgentName(existing.name)
          setNameDraft(existing.name)
          setAgentExists(true)
        }
        setReady(true)
      } catch (e) {
        console.error('[custom-agent] Init failed:', e)
        setReady(true)
      }
    }
    init()
  }, [projectPath, projectName])

  const RESERVED = ['__lead__', 'k2so-agent', 'pod-leader', 'default', 'legacy']
  const validateName = (n: string): string | null => {
    if (!/^[a-z][a-z0-9-]*[a-z0-9]$/.test(n) || n.length < 2) {
      return 'Lowercase letters, digits, hyphens only (min 2 chars, no leading/trailing hyphen)'
    }
    if (RESERVED.includes(n)) return `"${n}" is reserved`
    return null
  }

  const handleOpen = async (): Promise<void> => {
    const err = validateName(nameDraft)
    if (err) { setNameError(err); return }
    setNameError(null)

    if (!agentExists) {
      // First-time setup — create the agent with the user's chosen name.
      try {
        await invoke('k2so_agents_create', {
          projectPath,
          name: nameDraft,
          role: 'Custom agent — customize via the persona editor',
          agentType: 'custom',
        })
        setAgentName(nameDraft)
        setAgentExists(true)
        onOpenEditor(nameDraft)
      } catch (e) {
        setNameError(String(e))
      }
      return
    }
    // Already exists. If the user changed the name, note that rename
    // isn't wired yet — direct them to open the current persona for now.
    if (nameDraft !== agentName) {
      setNameError('Rename support for existing agents is coming in a later release — open the current persona for now, or use the CLI to rename.')
      setNameDraft(agentName)
      return
    }
    onOpenEditor(agentName)
  }

  return (
    <div>
      <div className="flex items-center gap-2">
        <div className="flex-1 min-w-0">
          <label className="block text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
            Agent name
          </label>
          <input
            type="text"
            value={nameDraft}
            onChange={(e) => { setNameDraft(e.target.value.toLowerCase()); setNameError(null) }}
            disabled={!ready || agentExists}
            placeholder={derived}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-60"
          />
        </div>
        <button
          onClick={handleOpen}
          disabled={!ready}
          className="px-5 py-1.5 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0 whitespace-nowrap self-end"
        >
          Manage Persona
        </button>
      </div>
      {nameError && (
        <p className="text-[10px] text-red-400 mt-1">{nameError}</p>
      )}
      <p className="text-[9px] text-[var(--color-text-muted)] mt-1">
        {agentExists
          ? `Define what ${agentName} does. Name is locked — rename support lands in a later release.`
          : `Give your custom agent a short name (defaults to the workspace name). The Persona editor creates the agent on first open.`}
      </p>
    </div>
  )
}

function K2SOAgentPersonaButton({ projectPath, projectName, onOpenEditor }: { projectPath: string; projectName: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const [ready, setReady] = useState(false)
  const [agentName, setAgentName] = useState('k2so-agent')

  // Ensure the K2SO agent exists for this workspace
  useEffect(() => {
    const ensure = async () => {
      try {
        const agents = await invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
        const existing = agents.find((a: any) => a.agentType === 'k2so')
        if (existing) {
          setAgentName(existing.name)
        } else {
          await invoke('k2so_agents_create', {
            projectPath,
            name: 'k2so-agent',
            role: 'K2SO planner — builds PRDs, milestones, and technical plans',
            agentType: 'k2so',
          })
        }
        setReady(true)
      } catch (e) {
        console.error('[k2so-agent] Init failed:', e)
        setReady(true)
      }
    }
    ensure()
  }, [projectPath, projectName])

  return (
    <div className="flex items-center justify-between gap-3">
      <div className="flex-1 min-w-0">
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Customize the K2SO agent&apos;s persona — add work sources, integrations, and project-specific context.
        </p>
      </div>
      <button
        onClick={() => onOpenEditor(agentName)}
        disabled={!ready}
        className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0"
      >
        Manage Persona
      </button>
    </div>
  )
}

// ── Connected Workspaces Panel ──────────────────────────────────────

interface WorkspaceRelation {
  id: string
  sourceProjectId: string
  targetProjectId: string
  relationType: string
  createdAt: string
}

function ConnectedWorkspacesPanel({ projectId }: { projectId: string }): React.JSX.Element {
  const [relations, setRelations] = useState<WorkspaceRelation[]>([])
  const [incoming, setIncoming] = useState<WorkspaceRelation[]>([])
  const [loading, setLoading] = useState(true)
  const [showAdd, setShowAdd] = useState(false)
  const [adding, setAdding] = useState(false)
  const [search, setSearch] = useState('')
  const projects = useProjectsStore((s) => s.projects)

  const fetchRelations = useCallback(async () => {
    try {
      const [outgoing, inc] = await Promise.all([
        invoke<WorkspaceRelation[]>('workspace_relations_list', { projectId }),
        invoke<WorkspaceRelation[]>('workspace_relations_list_incoming', { projectId }),
      ])
      setRelations(outgoing)
      setIncoming(inc)
    } catch {
      setRelations([])
      setIncoming([])
    } finally {
      setLoading(false)
    }
  }, [projectId])

  useEffect(() => {
    fetchRelations()
  }, [fetchRelations])

  // Projects available for connecting (exclude self and already-connected, sorted alphabetically)
  const connectedIds = useMemo(() => new Set(relations.map((r) => r.targetProjectId)), [relations])
  const availableProjects = useMemo(
    () => projects
      .filter((p) => p.id !== projectId && !connectedIds.has(p.id))
      .sort((a, b) => a.name.localeCompare(b.name)),
    [projects, projectId, connectedIds]
  )
  const filteredProjects = useMemo(
    () => search.trim()
      ? availableProjects.filter((p) => p.name.toLowerCase().includes(search.toLowerCase()))
      : availableProjects,
    [availableProjects, search]
  )

  const handleAdd = useCallback(async (targetProjectId: string) => {
    setAdding(true)
    try {
      await invoke('workspace_relations_create', { sourceProjectId: projectId, targetProjectId })
      setShowAdd(false)
      await fetchRelations()
    } catch (e) {
      console.error('[connected-workspaces] Create failed:', e)
    } finally {
      setAdding(false)
    }
  }, [projectId, fetchRelations])

  const handleRemove = useCallback(async (id: string) => {
    try {
      await invoke('workspace_relations_delete', { id })
      await fetchRelations()
    } catch (e) {
      console.error('[connected-workspaces] Delete failed:', e)
    }
  }, [fetchRelations])

  // Resolve target project details
  const projectsById = useMemo(() => {
    const map = new Map<string, typeof projects[number]>()
    for (const p of projects) map.set(p.id, p)
    return map
  }, [projects])

  return (
    <div>
      <div className="flex items-center justify-between mb-2">
        <div>
          <h3 className="text-xs font-medium text-[var(--color-text-primary)]">Connected Workspaces</h3>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Connect other workspaces so this agent can oversee or interact with them.
          </p>
        </div>
        {availableProjects.length > 0 && (
          <button
            onClick={() => { setShowAdd(!showAdd); setSearch('') }}
            title="Add connection"
            className="w-6 h-6 flex items-center justify-center text-sm leading-none bg-[var(--color-accent)] text-white cursor-pointer no-drag"
          >
            +
          </button>
        )}
      </div>

      {/* Add connection dropdown with search */}
      {showAdd && (
        <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
          <div className="px-3 py-1.5 border-b border-[var(--color-border)]">
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search workspaces..."
              autoFocus
              className="w-full bg-transparent text-xs text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
            />
          </div>
          <div className="max-h-[200px] overflow-y-auto">
            {filteredProjects.length === 0 ? (
              <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                {search.trim() ? 'No matching workspaces.' : 'No more workspaces available to connect.'}
              </div>
            ) : (
              filteredProjects.map((p) => (
                <button
                  key={p.id}
                  onClick={() => { handleAdd(p.id); setSearch('') }}
                  disabled={adding}
                  className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-white/[0.06] transition-colors no-drag cursor-pointer disabled:opacity-50 border-b border-[var(--color-border)] last:border-b-0"
                >
                  <span
                    className="w-2 h-2 flex-shrink-0 rounded-full"
                    style={{ backgroundColor: p.color || '#6b7280' }}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{p.name}</span>
                  {p.agentMode && p.agentMode !== 'off' && (
                    <span className="text-[9px] text-[var(--color-text-muted)] ml-auto flex-shrink-0">
                      {p.agentMode === 'custom' ? 'Custom' : p.agentMode === 'agent' ? 'K2SO' : 'Manager'}
                    </span>
                  )}
                </button>
              ))
            )}
          </div>
        </div>
      )}

      {/* Connected workspaces list */}
      {loading ? (
        <div className="text-[10px] text-[var(--color-text-muted)]">Loading...</div>
      ) : relations.length === 0 ? (
        <div className="text-[10px] text-[var(--color-text-muted)]">
          No connected workspaces yet.
        </div>
      ) : (
        <div className="border border-[var(--color-border)]">
          {relations.map((rel) => {
            const target = projectsById.get(rel.targetProjectId)
            return (
              <div
                key={rel.id}
                className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
              >
                <span
                  className="w-2 h-2 flex-shrink-0 rounded-full"
                  style={{ backgroundColor: target?.color || '#6b7280' }}
                />
                <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                  {target?.name || 'Unknown workspace'}
                </span>
                {target?.agentMode && target.agentMode !== 'off' && (
                  <span className="text-[9px] text-[var(--color-text-muted)]">
                    {target.agentMode === 'custom' ? 'Custom' : target.agentMode === 'agent' ? 'K2SO' : 'Manager'}
                  </span>
                )}
                <button
                  onClick={() => handleRemove(rel.id)}
                  className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-red-400 transition-colors no-drag cursor-pointer flex-shrink-0"
                  title="Remove connection"
                >
                  <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <line x1="1" y1="1" x2="7" y2="7" />
                    <line x1="7" y1="1" x2="1" y2="7" />
                  </svg>
                </button>
              </div>
            )
          })}
        </div>
      )}

      {/* Incoming connections (workspaces that connect TO this one) */}
      {!loading && incoming.length > 0 && (
        <>
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mt-4">
            Connected Agents
          </h3>
          <p className="text-[10px] text-[var(--color-text-muted)]">
            These agent workspaces have access to communicate with this workspace.
          </p>
          <div className="border border-[var(--color-border)]">
            {incoming.map((rel) => {
              const source = projectsById.get(rel.sourceProjectId)
              return (
                <div
                  key={rel.id}
                  className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
                >
                  <span
                    className="w-2 h-2 flex-shrink-0 rounded-full"
                    style={{ backgroundColor: source?.color || '#6b7280' }}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] flex-1 truncate">
                    {source?.name || 'Unknown workspace'}
                  </span>
                  {source?.agentMode && source.agentMode !== 'off' && (
                    <span className="text-[9px] text-[var(--color-text-muted)]">
                      {source.agentMode === 'custom' ? 'Custom' : source.agentMode === 'agent' ? 'K2SO' : 'Manager'}
                    </span>
                  )}
                </div>
              )
            })}
          </div>
        </>
      )}
    </div>
  )
}

function ProjectAgentsPanel({ projectPath, onOpenEditor }: { projectPath: string; onOpenEditor: (agentName: string) => void }): React.JSX.Element {
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [wsInboxCount, setWsInboxCount] = useState(0)
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [newName, setNewName] = useState('')
  const [newRole, setNewRole] = useState('')
  const [creating, setCreating] = useState(false)
  const nameInputRef = useRef<HTMLInputElement>(null)

  const fetchAgents = useCallback(async () => {
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', { projectPath })
      setAgents(result)
    } catch {
      setAgents([])
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  const fetchWsInbox = useCallback(async () => {
    try {
      const items = await invoke<unknown[]>('k2so_agents_workspace_inbox_list', { projectPath })
      setWsInboxCount(items.length)
    } catch {
      setWsInboxCount(0)
    }
  }, [projectPath])

  useEffect(() => {
    fetchAgents()
    fetchWsInbox()
  }, [fetchAgents, fetchWsInbox])

  useEffect(() => {
    if (showCreate) {
      requestAnimationFrame(() => nameInputRef.current?.focus())
    }
  }, [showCreate])

  const handleCreate = useCallback(async () => {
    if (!newName.trim() || !newRole.trim()) return
    setCreating(true)
    try {
      await invoke('k2so_agents_create', {
        projectPath,
        name: newName.trim().toLowerCase().replace(/\s+/g, '-'),
        role: newRole.trim(),
      })
      setNewName('')
      setNewRole('')
      setShowCreate(false)
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Create failed:', e)
    } finally {
      setCreating(false)
    }
  }, [projectPath, newName, newRole, fetchAgents])

  const handleDelete = useCallback(async (name: string) => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Delete Agent "${name}"?`,
      message: 'This will delete the agent and all its work items. This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await invoke('k2so_agents_delete', { projectPath, name })
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Delete failed:', e)
    }
  }, [projectPath, fetchAgents])

  const handleLaunch = useCallback(async (name: string) => {
    try {
      const launchInfo = await invoke<{
        command: string
        args: string[]
        cwd: string
        agentName: string
      }>('k2so_agents_build_launch', { projectPath, agentName: name })

      const tabsStore = useTabsStore.getState()
      tabsStore.addTab(launchInfo.cwd, {
        title: `Agent: ${launchInfo.agentName}`,
        command: launchInfo.command,
        args: launchInfo.args,
      })

      // Close settings so the user can see the launched agent
      useSettingsStore.getState().closeSettings()
    } catch (e) {
      console.error('[agents] Launch failed:', e)
    }
  }, [projectPath])

  const manager = agents.find((a) => a.isCoordinator)
  const agentTemplates = agents.filter((a) => !a.isCoordinator)
  const totalDelegated = agentTemplates.reduce((sum, a) => sum + a.inboxCount + a.activeCount, 0)
  const totalDone = agentTemplates.reduce((sum, a) => sum + a.doneCount, 0)

  const openAgentSettings = (agentName: string) => {
    useTabsStore.getState().openAgentPane(agentName, projectPath)
    useSettingsStore.getState().closeSettings()
  }

  const AgentListItem = ({ agent }: { agent: K2soAgentInfo }) => (
    <div className="px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
      <div className="flex items-center justify-between">
        <div className="flex-1 min-w-0 mr-3">
          <div className="flex items-center">
            <span className="text-xs font-medium text-[var(--color-text-primary)] flex-shrink-0">{agent.name}</span>
            <div className="flex items-center justify-end gap-1.5 text-[10px] text-[var(--color-text-muted)] flex-1 ml-2">
              {agent.inboxCount > 0 && <span title="Inbox items">{agent.inboxCount} inbox</span>}
              {agent.activeCount > 0 && <span className="text-yellow-400" title="Active">{agent.activeCount} active</span>}
              {agent.doneCount > 0 && <span className="text-green-400" title="Done">{agent.doneCount} done</span>}
            </div>
          </div>
          <p className="text-[10px] text-[var(--color-text-muted)] truncate mt-0.5">{agent.role}</p>
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          <button
            onClick={() => onOpenEditor(agent.name)}
            className="px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer"
            title="Manage agent persona"
          >
            Manage Persona
          </button>
          <button
            onClick={() => handleDelete(agent.name)}
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-red-400 transition-colors no-drag cursor-pointer"
            title="Delete agent"
          >
            <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="1" y1="1" x2="7" y2="7" />
              <line x1="7" y1="1" x2="1" y2="7" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  )

  return (
    <div className="space-y-3">
      {/* Manager section */}
      {manager && (
        <div>
          <h3 className="text-[10px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-1">
            Workspace Manager
          </h3>
          <div className="border border-[var(--color-accent)]/30">
            <div className="px-3 py-2">
              <div className="flex items-center justify-between">
                <div className="flex-1 min-w-0 mr-3">
                  <div className="flex items-center">
                    <span className="text-xs font-medium text-[var(--color-text-primary)] flex-shrink-0">{manager.name}</span>
                    <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 ml-1.5 flex-shrink-0">
                      MANAGER
                    </span>
                    <div className="flex items-center justify-end gap-1.5 text-[10px] flex-1 ml-2">
                      {wsInboxCount > 0 && (
                        <span className="text-[var(--color-accent)]" title="Undelegated work in workspace inbox">{wsInboxCount} undelegated</span>
                      )}
                      {totalDelegated > 0 && (
                        <span className="text-yellow-400" title="Work assigned to agents">{totalDelegated} delegated</span>
                      )}
                      {totalDone > 0 && (
                        <span className="text-green-400" title="Completed, awaiting review">{totalDone} done</span>
                      )}
                    </div>
                  </div>
                  <p className="text-[10px] text-[var(--color-text-muted)] truncate mt-0.5">{manager.role}</p>
                </div>
                <div className="flex items-center gap-1 flex-shrink-0">
                  <button
                    onClick={() => onOpenEditor(manager.name)}
                    className="px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer"
                    title="Manage workspace manager persona"
                  >
                    Manage Persona
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Project Context */}
      <div>
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
          Project Context
        </h3>
        <div className="border border-[var(--color-border)] px-3 py-2">
          <div className="flex items-center justify-between">
            <div className="flex-1 min-w-0 mr-3">
              <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                Shared knowledge about this codebase that all agents receive at launch — tech stack, conventions, key directories.
              </p>
            </div>
            <button
              onClick={() => onOpenEditor('__project_context__')}
              className="px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer flex-shrink-0"
              title="Edit shared project context"
            >
              Manage Project Context
            </button>
          </div>
        </div>
      </div>

      {/* Workspace Wake-up retired in 0.32.6. Its content migrated to the
          per-workspace `triage` heartbeat row (edit via the Heartbeats
          list above — each row has its own WAKEUP.md now). See the
          migrate_or_scaffold_lead_heartbeat startup pass. */}

      {/* Agent Templates section */}
      <div>
        <div className="flex items-center justify-between mb-1">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
            Agent Templates
            {agentTemplates.length > 0 && (
              <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{agentTemplates.length}</span>
            )}
          </h3>
          <button
            onClick={() => setShowCreate(!showCreate)}
            className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
          >
            {showCreate ? 'Cancel' : '+ New Agent'}
          </button>
        </div>

        {/* Create form */}
        {showCreate && (
          <div className="border border-[var(--color-border)] p-3 space-y-2 mb-2">
            <input
              ref={nameInputRef}
              type="text"
              placeholder="Agent name (e.g. backend-eng)"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
              onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
            />
            <input
              type="text"
              placeholder="Role (e.g. Backend engineering and API development)"
              value={newRole}
              onChange={(e) => setNewRole(e.target.value)}
              className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
              onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
            />
            <button
              onClick={handleCreate}
              disabled={creating || !newName.trim() || !newRole.trim()}
              className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {creating ? 'Creating...' : 'Create Agent'}
            </button>
          </div>
        )}

        {/* Agent list */}
        {loading ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">Loading agents...</p>
        ) : agentTemplates.length === 0 && !showCreate ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            No agents configured. Create one to enable autonomous work.
          </p>
        ) : (
          <div className="border border-[var(--color-border)]">
            {agentTemplates.map((agent) => (
              <AgentListItem key={agent.name} agent={agent} />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

// ── Cursor IDE Chat Migration Panel ─────────────────────────────────

interface CursorIdeSession {
  composerId: string
  name: string
  createdAt: number
  lastUpdatedAt: number
  mode: string
  alreadyMigrated: boolean
  migratable: boolean
}

function CursorMigrationPanel({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [sessions, setSessions] = useState<CursorIdeSession[]>([])
  const [loading, setLoading] = useState(true)
  const [migrating, setMigrating] = useState(false)
  const [migratingIds, setMigratingIds] = useState<Set<string>>(new Set())
  const [justMigratedIds, setJustMigratedIds] = useState<Set<string>>(new Set())
  const [error, setError] = useState<string | null>(null)

  const fetchIdeSessions = useCallback(async () => {
    try {
      const result = await invoke<CursorIdeSession[]>('chat_history_discover_ide_sessions', { projectPath })
      setSessions(result)
    } catch {
      setSessions([])
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetchIdeSessions()
  }, [fetchIdeSessions])

  const unmigratedSessions = sessions.filter((s) => !s.alreadyMigrated && !justMigratedIds.has(s.composerId) && s.migratable)
  const migratedSessions = sessions.filter((s) => s.alreadyMigrated || justMigratedIds.has(s.composerId))
  const nonMigratableSessions = sessions.filter((s) => !s.migratable && !s.alreadyMigrated)

  const handleMigrateAll = useCallback(async () => {
    if (unmigratedSessions.length === 0) return
    setMigrating(true)
    setError(null)

    let succeeded = 0
    let failed = 0

    // Migrate one at a time so the UI updates per-session
    for (const session of unmigratedSessions) {
      setMigratingIds(new Set([session.composerId]))
      try {
        const count = await invoke<number>('chat_history_migrate_ide_sessions', {
          projectPath,
          composerIds: [session.composerId],
        })
        if (count > 0) {
          succeeded++
          setJustMigratedIds((prev) => new Set([...prev, session.composerId]))
        } else {
          failed++
        }
      } catch {
        failed++
      }
    }

    if (failed > 0) {
      setError(`${succeeded} migrated, ${failed} failed (missing conversation data)`)
    }
    setMigrating(false)
    setMigratingIds(new Set())
    await fetchIdeSessions()
  }, [unmigratedSessions, projectPath, fetchIdeSessions])

  if (loading) return null
  if (sessions.length === 0) return null

  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
        Cursor IDE Conversations
        <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{sessions.length}</span>
      </h3>

      <p className="text-[10px] text-[var(--color-text-muted)]">
        Migrate conversations from the Cursor IDE to CLI format so they can be resumed in K2SO terminals.
      </p>

      {/* Session list */}
      <div className="border border-[var(--color-border)] max-h-[250px] overflow-y-auto">
        {sessions.map((session, i) => {
          const isMigrated = session.alreadyMigrated || justMigratedIds.has(session.composerId)
          const isCurrentlyMigrating = migratingIds.has(session.composerId)
          const date = new Date(session.lastUpdatedAt || session.createdAt)
          const dateStr = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })

          return (
            <div
              key={session.composerId}
              className={`flex items-center gap-2 px-3 py-1.5 text-xs ${
                i < sessions.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <AgentIcon agent="Cursor Agent" size={12} />
              <span className={`flex-1 truncate ${isMigrated ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}>
                {session.name || 'Untitled'}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                {dateStr}
              </span>
              {isCurrentlyMigrating ? (
                <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0 animate-pulse">
                  migrating...
                </span>
              ) : isMigrated ? (
                <span className="text-[10px] text-green-400 flex-shrink-0">
                  migrated
                </span>
              ) : !session.migratable ? (
                <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0 opacity-50">
                  chat only
                </span>
              ) : (
                <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0">
                  pending
                </span>
              )}
            </div>
          )
        })}
      </div>

      {/* Error */}
      {error && (
        <p className="text-[10px] text-red-400">{error}</p>
      )}

      {/* Status + button */}
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-[var(--color-text-muted)]">
          {migratedSessions.length > 0 && (
            <span className="text-green-400">{migratedSessions.length} migrated</span>
          )}
          {migratedSessions.length > 0 && unmigratedSessions.length > 0 && ' · '}
          {unmigratedSessions.length > 0 && (
            <span>{unmigratedSessions.length} pending</span>
          )}
          {nonMigratableSessions.length > 0 && (
            <span> · {nonMigratableSessions.length} chat-only</span>
          )}
          {migratedSessions.length > 0 && unmigratedSessions.length === 0 && nonMigratableSessions.length === 0 && (
            <span> — all conversations available in Chat History</span>
          )}
        </span>

        {unmigratedSessions.length > 0 && (
          <button
            onClick={handleMigrateAll}
            disabled={migrating}
            className="px-3 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer disabled:opacity-50 no-drag"
          >
            {migrating
              ? `Migrating ${migratingIds.size}...`
              : `Migrate ${unmigratedSessions.length}`}
          </button>
        )}
      </div>
    </div>
  )
}




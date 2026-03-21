import { useEffect } from 'react'
import { useTabsStore } from '@/stores/tabs'
import { usePresetsStore } from '@/stores/presets'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import type { TerminalPane } from '@/stores/tabs'

/**
 * Registers global keyboard shortcuts for terminal tab/pane management.
 *
 * - Cmd+T         — New tab
 * - Cmd+W         — Close active tab
 * - Cmd+D         — Split pane vertically
 * - Cmd+Shift+D   — Split pane horizontally
 * - Cmd+Alt+Left  — Previous tab
 * - Cmd+Alt+Right — Next tab
 * - Cmd+1-9       — Switch to workspace by index
 * - Cmd+K         — Clear active terminal (sends clear sequence)
 * - Ctrl+1-9      — Launch preset by position
 */
export function useTerminalShortcuts(cwd: string): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      // Ctrl+1-9: launch preset by position
      if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
        const num = parseInt(e.key, 10)
        if (num >= 1 && num <= 9) {
          e.preventDefault()
          const presetsState = usePresetsStore.getState()
          const enabledPresets = presetsState.presets.filter((p) => p.enabled)
          const targetIdx = num - 1
          if (targetIdx < enabledPresets.length) {
            presetsState.launchPreset(enabledPresets[targetIdx].id, cwd, 'tab')
          }
          return
        }
      }

      // Only handle Cmd (Meta) shortcuts
      if (!e.metaKey) return

      const state = useTabsStore.getState()

      switch (e.key) {
        case 't': {
          if (e.shiftKey || e.altKey) return
          e.preventDefault()
          state.addTab(cwd)
          break
        }

        case 'w': {
          if (e.shiftKey || e.altKey) return
          e.preventDefault()
          if (state.activeTabId) {
            state.removeTab(state.activeTabId)
          }
          break
        }

        case 'd': {
          e.preventDefault()
          const activeTab = state.tabs.find((t) => t.id === state.activeTabId)
          if (!activeTab) return

          // Find the first pane to split from
          const firstPaneId = getFirstLeaf(activeTab.mosaicTree)
          if (!firstPaneId) return

          const newPaneId = crypto.randomUUID()
          const newPane: TerminalPane = { type: 'terminal', terminalId: newPaneId, cwd }
          const direction = e.shiftKey ? 'row' : 'column'

          state.splitPane(activeTab.id, firstPaneId, newPaneId, newPane, direction)
          break
        }

        case 'ArrowLeft': {
          if (!e.altKey) return
          e.preventDefault()
          const currentIdx = state.tabs.findIndex((t) => t.id === state.activeTabId)
          if (currentIdx > 0) {
            state.setActiveTab(state.tabs[currentIdx - 1].id)
          }
          break
        }

        case 'ArrowRight': {
          if (!e.altKey) return
          e.preventDefault()
          const curIdx = state.tabs.findIndex((t) => t.id === state.activeTabId)
          if (curIdx < state.tabs.length - 1) {
            state.setActiveTab(state.tabs[curIdx + 1].id)
          }
          break
        }

        case 'k': {
          if (e.shiftKey || e.altKey) return
          // Cmd+K: let the terminal handle clear — we don't intercept this
          // The terminal component forwards keystrokes to the pty
          break
        }

        default: {
          // Cmd+1-9 — switch to workspace by index
          const num = parseInt(e.key, 10)
          if (num >= 1 && num <= 9 && !e.shiftKey && !e.altKey) {
            e.preventDefault()
            const projectsState = useProjectsStore.getState()
            const focusState = useFocusGroupsStore.getState()

            // Apply the same focus group filter as the sidebar
            let filteredProjects = projectsState.projects
            if (focusState.focusGroupsEnabled && focusState.activeFocusGroupId !== null) {
              filteredProjects = filteredProjects.filter(
                (p) => p.focusGroupId === focusState.activeFocusGroupId
              )
            }

            const targetIdx = num - 1
            if (targetIdx < filteredProjects.length) {
              const project = filteredProjects[targetIdx]
              const firstWorkspace = project.workspaces[0]
              if (firstWorkspace) {
                projectsState.setActiveWorkspace(project.id, firstWorkspace.id)
              }
            }
          }
          break
        }
      }
    }

    window.addEventListener('keydown', handler)
    return () => {
      window.removeEventListener('keydown', handler)
    }
  }, [cwd])
}

// ── Helpers ──────────────────────────────────────────────────────────────

function getFirstLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getFirstLeaf((tree as { first: unknown }).first)
  }
  return null
}

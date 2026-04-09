import React from 'react'
import { useEffect, useState, useCallback, useRef, useMemo } from 'react'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import type { SettingsSection, TerminalSettings } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import type { LinkClickMode } from '@/stores/terminal-settings'
import { invoke } from '@tauri-apps/api/core'
import { useAssistantStore } from '@/stores/assistant'
import { useTabsStore } from '@/stores/tabs'
import { usePanelsStore } from '@/stores/panels'
import IconCropDialog from './IconCropDialog'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import {
  HOTKEYS,
  RESERVED_KEYS,
  formatKeyCombo,
  keyEventToCombo,
  isReservedKey
} from '@shared/hotkeys'
import type { HotkeyDefinition } from '@shared/hotkeys'
import { showContextMenu } from '@/lib/context-menu'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { EDITOR_THEMES, EDITOR_FONTS, CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from './CustomThemeCreator'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { AIFileEditor } from '@/components/AIFileEditor/AIFileEditor'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { KeyCombo } from '@/components/KeySymbol'
import { useClaudeAuthStore } from '@/stores/claude-auth'
import type { ClaudeAuthState } from '@/stores/claude-auth'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import { useUpdateStore } from '@/stores/update'
import {
  useTimerStore,
  formatTimestamp,
  formatDuration,
  type TimeEntry,
  type CountdownThemeConfig,
} from '@/stores/timer'

// ── Error Boundary ───────────────────────────────────────────────────
class SectionErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[Settings] Section render error:', error, info.componentStack)
  }

  render(): React.ReactNode {
    if (this.state.error) {
      return (
        <div className="max-w-xl p-4">
          <h2 className="text-sm font-medium text-red-400 mb-2">Something went wrong</h2>
          <p className="text-xs text-[var(--color-text-muted)] mb-3">
            This section failed to render. Try restarting the app.
          </p>
          <pre className="text-[10px] text-red-400/70 bg-red-500/5 border border-red-500/20 p-2 overflow-x-auto whitespace-pre-wrap">
            {this.state.error.message}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            className="mt-3 px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
          >
            Try Again
          </button>
        </div>
      )
    }
    return this.props.children
  }
}

// ── Section nav items ────────────────────────────────────────────────
const SECTIONS: { id: SettingsSection; label: string; agenticOnly?: boolean }[] = [
  { id: 'general', label: 'General' },
  { id: 'projects', label: 'Workspaces' },
  { id: 'workspace-states', label: 'Workspace States', agenticOnly: true },
  { id: 'agent-skills', label: 'Agent Skills', agenticOnly: true },
  { id: 'terminal', label: 'Terminal' },
  { id: 'code-editor', label: 'Code Editor' },
  { id: 'editors-agents', label: 'Editors & Agents' },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'timer', label: 'Timer' },
  { id: 'companion', label: 'Mobile Companion' },
]

// ── Main Settings component ──────────────────────────────────────────
export default function Settings(): React.JSX.Element {
  const activeSection = useSettingsStore((s) => s.activeSection)
  const setSection = useSettingsStore((s) => s.setSection)
  const closeSettings = useSettingsStore((s) => s.closeSettings)

  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        closeSettings()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [closeSettings])

  return (
    <div className="flex h-full w-full min-h-0 bg-[var(--color-bg)]">
      {/* Left nav */}
      <div className="w-48 flex-shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-surface)] flex flex-col min-h-0">
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
          <span className="text-xs font-medium text-[var(--color-text-secondary)] uppercase tracking-wider">
            Settings
          </span>
        </div>
        <nav className="flex-1 py-1 overflow-y-auto">
          {SECTIONS.filter((s) => !s.agenticOnly || useSettingsStore.getState().agenticSystemsEnabled).map((s) => (
            <button
              key={s.id}
              onClick={() => setSection(s.id)}
              className={`w-full text-left px-4 py-1.5 text-xs no-drag cursor-pointer transition-colors ${
                activeSection === s.id
                  ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
              }`}
            >
              {s.label}
            </button>
          ))}
        </nav>
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex-shrink-0">
          <button
            onClick={closeSettings}
            className="flex items-center gap-2 text-xs text-[var(--color-text-primary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          >
            &larr; Back
            <span className="text-[10px] text-[var(--color-text-muted)]">Esc</span>
          </button>
        </div>
      </div>

      {/* Content area */}
      <div className={`flex-1 min-h-0 relative ${activeSection === 'projects' ? 'overflow-hidden p-0' : 'overflow-y-auto p-6'}`}>
        {activeSection === 'general' && <GeneralSection />}
        {activeSection === 'terminal' && <TerminalSection />}
        {activeSection === 'code-editor' && <CodeEditorSettingsSection />}
        {activeSection === 'editors-agents' && <EditorsAgentsSection />}
        {activeSection === 'keybindings' && <KeybindingsSection />}
        {activeSection === 'timer' && (
          <SectionErrorBoundary>
            <TimerSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'companion' && (
          <SectionErrorBoundary>
            <CompanionSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'projects' && (
          <SectionErrorBoundary>
            <ProjectsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'workspace-states' && (
          <SectionErrorBoundary>
            <WorkspaceStatesSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'agent-skills' && (
          <SectionErrorBoundary>
            <AgentSkillsSection />
          </SectionErrorBoundary>
        )}
      </div>
    </div>
  )
}

// ── General Section ──────────────────────────────────────────────────
// ── Workspace States Section ──────────────────────────────────────────

interface StateData {
  id: string
  name: string
  description: string | null
  isBuiltIn: number
  capFeatures: string
  capIssues: string
  capCrashes: string
  capSecurity: string
  capAudits: string
  heartbeat: number
  sortOrder: number
}

const CAP_STATES = ['auto', 'gated', 'off'] as const
const CAP_LABELS: Record<string, string> = {
  auto: 'Auto',
  gated: 'Gated',
  off: 'Off',
}
const CAP_COLORS: Record<string, string> = {
  auto: 'text-green-400',
  gated: 'text-amber-400',
  off: 'text-[var(--color-text-muted)]',
}

const CAPABILITIES = [
  { key: 'capFeatures' as const, label: 'Features', desc: 'New functionality and enhancements' },
  { key: 'capIssues' as const, label: 'Issues', desc: 'Bug fixes from submitted issues' },
  { key: 'capCrashes' as const, label: 'Crashes', desc: 'Automatic crash report fixes' },
  { key: 'capSecurity' as const, label: 'Security', desc: 'Automatic security patches' },
  { key: 'capAudits' as const, label: 'Audits', desc: 'Scheduled code reviews' },
]

function WorkspaceStatesSection(): React.JSX.Element {
  const [states, setStates] = useState<StateData[]>([])
  const [editingState, setEditingState] = useState<StateData | null>(null)
  const [creating, setCreating] = useState(false)

  const loadStates = useCallback(async () => {
    try {
      const list = await invoke<StateData[]>('states_list')
      setStates(list)
    } catch (err) {
      console.error('[states] Failed to load:', err)
    }
  }, [])

  useEffect(() => { loadStates() }, [loadStates])

  const handleSave = async (entry: StateData) => {
    try {
      if (creating) {
        await invoke('states_create', {
          name: entry.name,
          description: entry.description,
          capFeatures: entry.capFeatures,
          capIssues: entry.capIssues,
          capCrashes: entry.capCrashes,
          capSecurity: entry.capSecurity,
          capAudits: entry.capAudits,
          heartbeat: entry.heartbeat === 1,
        })
      } else {
        await invoke('states_update', {
          id: entry.id,
          name: entry.name,
          description: entry.description,
          capFeatures: entry.capFeatures,
          capIssues: entry.capIssues,
          capCrashes: entry.capCrashes,
          capSecurity: entry.capSecurity,
          capAudits: entry.capAudits,
          heartbeat: entry.heartbeat === 1,
        })
      }
      setEditingState(null)
      setCreating(false)
      loadStates()
    } catch (err) {
      console.error('[states] Save failed:', err)
    }
  }

  const handleDelete = async (id: string) => {
    try {
      await invoke('states_delete', { id })
      loadStates()
    } catch (err) {
      console.error('[states] Delete failed:', err)
    }
  }

  const handleNew = () => {
    setCreating(true)
    setEditingState({
      id: '',
      name: 'New State',
      description: '',
      isBuiltIn: 0,
      capFeatures: 'gated',
      capIssues: 'gated',
      capCrashes: 'auto',
      capSecurity: 'auto',
      capAudits: 'off',
      heartbeat: 1,
      sortOrder: states.length,
    })
  }

  // Editor view
  if (editingState) {
    return (
      <div className="max-w-lg">
        <button
          onClick={() => { setEditingState(null); setCreating(false) }}
          className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] mb-4 no-drag cursor-pointer"
        >
          &larr; Back to states
        </button>

        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">
          {creating ? 'Create State' : `Edit: ${editingState.name}`}
        </h2>

        {/* Name */}
        <div className="mb-3">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">Name</label>
          <input
            value={editingState.name}
            onChange={(e) => setEditingState({ ...editingState, name: e.target.value })}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
            disabled={editingState.isBuiltIn === 1}
          />
        </div>

        {/* Description */}
        <div className="mb-4">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">Description</label>
          <input
            value={editingState.description || ''}
            onChange={(e) => setEditingState({ ...editingState, description: e.target.value })}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
          />
        </div>

        {/* Capabilities */}
        <div className="mb-4">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-2">Capabilities</label>
          <div className="space-y-2">
            {CAPABILITIES.map((cap) => (
              <div key={cap.key} className="flex items-center justify-between py-1.5 px-3 bg-[var(--color-bg-elevated)] border border-[var(--color-border)]">
                <div>
                  <span className="text-xs text-[var(--color-text-primary)]">{cap.label}</span>
                  <p className="text-[9px] text-[var(--color-text-muted)]">{cap.desc}</p>
                </div>
                <div className="flex gap-0.5">
                  {CAP_STATES.map((state) => (
                    <button
                      key={state}
                      onClick={() => setEditingState({ ...editingState, [cap.key]: state })}
                      className={`px-2 py-0.5 text-[9px] border transition-colors cursor-pointer no-drag ${
                        editingState[cap.key] === state
                          ? `border-[var(--color-accent)] ${CAP_COLORS[state]} bg-[var(--color-accent)]/10`
                          : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                      }`}
                    >
                      {CAP_LABELS[state]}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Heartbeat toggle */}
        <div className="flex items-center justify-between mb-6 py-1.5 px-3 bg-[var(--color-bg-elevated)] border border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-primary)]">Heartbeat</span>
            <p className="text-[9px] text-[var(--color-text-muted)]">Whether agents can wake up automatically</p>
          </div>
          <button
            onClick={() => setEditingState({ ...editingState, heartbeat: editingState.heartbeat ? 0 : 1 })}
            className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
              editingState.heartbeat ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span className={`w-3 h-3 bg-white block transition-transform ${
              editingState.heartbeat ? 'translate-x-4.5' : 'translate-x-0.5'
            }`} />
          </button>
        </div>

        {/* Save button */}
        <button
          onClick={() => handleSave(editingState)}
          className="px-4 py-1.5 text-xs bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer"
        >
          {creating ? 'Create State' : 'Save Changes'}
        </button>
      </div>
    )
  }

  // List view
  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Workspace States</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Define capability states that control what agents can do automatically per workspace.
          </p>
        </div>
        <button
          onClick={handleNew}
          className="px-3 py-1 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
        >
          + New State
        </button>
      </div>

      {/* Capability columns */}
      <div className="mb-4">
        <span className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider block mb-1.5">Capability Columns</span>
        <div className="space-y-1">
          {CAPABILITIES.map((cap) => (
            <div key={cap.key} className="flex items-baseline gap-2 text-[11px]">
              <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">{cap.label}</span>
              <span className="text-[var(--color-text-muted)]">{cap.desc}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Status levels */}
      <div className="mb-4">
        <span className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider block mb-1.5">Status Levels</span>
        <div className="space-y-1">
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-green-400 flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Auto</span>
            <span className="text-[var(--color-text-muted)]">Agents handle this automatically without human approval</span>
          </div>
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-amber-400 flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Gated</span>
            <span className="text-[var(--color-text-muted)]">Requires human approval before agents act</span>
          </div>
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-[var(--color-text-muted)] flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Off</span>
            <span className="text-[var(--color-text-muted)]">Not functioning for this capability</span>
          </div>
        </div>
      </div>

      {/* State comparison table */}
      <div className="border border-[var(--color-border)] overflow-hidden">
        {/* Header */}
        <div className="grid gap-0 text-[var(--color-text-muted)] bg-[var(--color-bg-surface)]" style={{ gridTemplateColumns: '2fr repeat(5, 100px)' }}>
          <div className="px-4 py-2">
            <span className="text-[11px] font-medium">State</span>
          </div>
          {CAPABILITIES.map((cap) => (
            <div key={cap.key} className="px-2 py-2 text-center">
              <span className="text-[11px] font-medium">{cap.label}</span>
            </div>
          ))}
        </div>

        {/* Rows */}
        {states.map((entry) => (
          <div
            key={entry.id}
            className="grid gap-0 border-t border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)]/50 group"
            style={{ gridTemplateColumns: '2fr repeat(5, 100px)' }}
          >
            <div className="px-4 py-3 flex items-start gap-2">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className={`w-2 h-2 rounded-full flex-shrink-0 ${entry.heartbeat ? 'bg-green-400' : 'bg-[var(--color-text-muted)]'}`} />
                  <span className="text-[12px] font-medium text-[var(--color-text-primary)]">{entry.name}</span>
                  {entry.isBuiltIn === 1 && (
                    <span className="text-[9px] text-[var(--color-text-muted)] px-1 py-0.5 border border-[var(--color-border)] flex-shrink-0">DEFAULT</span>
                  )}
                </div>
                {entry.description && (
                  <p className="text-[11px] text-[var(--color-text-muted)] mt-1 pl-4 leading-relaxed">{entry.description}</p>
                )}
              </div>
              <div className="flex gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0 mt-0.5">
                <button
                  onClick={() => setEditingState(entry)}
                  className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-accent)] no-drag cursor-pointer"
                >
                  Edit
                </button>
                {entry.isBuiltIn === 0 && (
                  <button
                    onClick={() => handleDelete(entry.id)}
                    className="text-[10px] text-red-400/50 hover:text-red-400 no-drag cursor-pointer"
                  >
                    Delete
                  </button>
                )}
              </div>
            </div>
            {CAPABILITIES.map((cap) => {
              const capState = entry[cap.key] as string
              return (
                <div key={cap.key} className="px-2 py-3 flex items-center justify-center">
                  <span className={`text-[11px] font-medium ${CAP_COLORS[capState] || ''}`}>
                    {CAP_LABELS[capState] || capState}
                  </span>
                </div>
              )
            })}
          </div>
        ))}
      </div>
    </div>
  )
}

function GeneralSection(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  const [confirming, setConfirming] = useState(false)
  const [currentVersion, setCurrentVersion] = useState<string>('')
  const updateStatus = useUpdateStore((s) => s.status)
  const updateVersion = useUpdateStore((s) => s.version)
  const updateProgress = useUpdateStore((s) => s.progress)
  const updateError = useUpdateStore((s) => s.error)

  // Load current version on mount
  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    await checkForUpdate(true)
  }, [])

  // Auto-check for updates when navigated here from the update toast
  useEffect(() => {
    if (useSettingsStore.getState().pendingUpdateCheck) {
      useSettingsStore.setState({ pendingUpdateCheck: false })
      handleCheckUpdate()
    }
  }, [handleCheckUpdate])

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">General</h2>

      <div className="space-y-4">
        {/* Version & Update */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">App Version</span>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateStatus === 'available' ? '#eab308' : '#4ade80' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{currentVersion || '...'}
              </span>
            </div>
            {updateStatus === 'idle' && (
              <button
                onClick={handleCheckUpdate}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
              >
                Check for Updates
              </button>
            )}
            {updateStatus === 'checking' && (
              <span className="text-[10px] text-[var(--color-text-muted)]">Checking...</span>
            )}
          </div>
        </div>

        {/* Update available */}
        {updateStatus === 'available' && updateVersion && (
          <div className="flex items-center justify-between p-3 bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">K2SO v{updateVersion} is available</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">You&apos;re on v{currentVersion}</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().startDownload()}
            >
              Download & Install
            </button>
          </div>
        )}

        {/* Downloading */}
        {updateStatus === 'downloading' && (
          <div className="p-3 border border-[var(--color-border)]">
            <div className="flex items-center justify-between mb-2">
              <span className="text-xs text-[var(--color-text-primary)]">Downloading v{updateVersion}...</span>
              <span className="text-[10px] tabular-nums text-[var(--color-text-muted)]">{updateProgress}%</span>
            </div>
            <div className="h-1.5 bg-[var(--color-border)] overflow-hidden">
              <div
                className="h-full bg-[var(--color-accent)] transition-all duration-300"
                style={{ width: `${updateProgress}%` }}
              />
            </div>
          </div>
        )}

        {/* Ready to install */}
        {updateStatus === 'ready' && (
          <div className="flex items-center justify-between p-3 bg-green-500/10 border border-green-500/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">v{updateVersion} is ready to install</p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">The app will restart after installation</p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-green-500 text-white hover:bg-green-600 transition-colors no-drag cursor-pointer"
              onClick={() => useUpdateStore.getState().installAndRelaunch()}
            >
              Install & Relaunch
            </button>
          </div>
        )}

        {/* Error */}
        {updateStatus === 'error' && (
          <div className="p-3 border border-red-500/30 bg-red-500/5">
            <p className="text-[10px] text-red-400">{updateError}</p>
            <div className="flex items-center gap-2 mt-2">
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                onClick={handleCheckUpdate}
              >
                Retry
              </button>
              <button
                className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
                onClick={() => {
                  const tag = updateVersion ? `v${updateVersion}` : 'latest'
                  invoke('plugin:opener|open_url', { url: `https://github.com/Alakazam-211/K2SO/releases/tag/${tag}` }).catch(() => {
                    window.open(`https://github.com/Alakazam-211/K2SO/releases/tag/${tag}`)
                  })
                }}
              >
                Download
              </button>
            </div>
          </div>
        )}

        {/* CLI Version — right under App Version so it feels like part of the app */}
        <CLIVersionRow />

        {/* Agentic Systems master switch */}
        <AgenticSystemsToggle />

        {/* Claude Auth Auto-Refresh */}
        <ClaudeAuthRefreshRow />

        {/* AI Workspace Assistant (Cmd+L) — core feature, belongs in General */}
        <LocalLLMSettings />

        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Config Location</span>
          <span className="text-xs text-[var(--color-text-muted)]">~/.k2so/settings.json</span>
        </div>

        <div className="pt-4">
          {confirming ? (
            <div className="flex items-center gap-2">
              <span className="text-xs text-red-400">Reset all settings to defaults?</span>
              <button
                onClick={() => {
                  resetAllSettings()
                  setConfirming(false)
                }}
                className="px-3 py-1 text-xs bg-red-500/20 text-red-400 border border-red-500/40 hover:bg-red-500/30 no-drag cursor-pointer"
              >
                Confirm
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              className="px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer"
            >
              Reset All Settings
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

// ── Terminal Section ─────────────────────────────────────────────────
function TerminalSection(): React.JSX.Element {
  const terminal = useSettingsStore((s) => s.terminal)
  const updateTerminalSettings = useSettingsStore((s) => s.updateTerminalSettings)
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)
  const setLinkClickMode = useTerminalSettingsStore((s) => s.setLinkClickMode)
  const openLinksInSplitPane = useTerminalSettingsStore((s) => s.openLinksInSplitPane)
  const setOpenLinksInSplitPane = useTerminalSettingsStore((s) => s.setOpenLinksInSplitPane)

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">Terminal</h2>

      <div className="space-y-4">
        {/* Font Family */}
        <SettingRow label="Font Family">
          <input
            type="text"
            value={terminal.fontFamily}
            onChange={(e) => updateTerminalSettings({ fontFamily: e.target.value })}
            className="w-64 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
          />
        </SettingRow>

        {/* Font Size */}
        <SettingRow label="Font Size">
          <div className="flex items-center gap-3">
            <input
              type="range"
              min={10}
              max={24}
              step={1}
              value={terminal.fontSize}
              onChange={(e) => updateTerminalSettings({ fontSize: parseInt(e.target.value, 10) })}
              className="w-40 no-drag k2so-slider"
            />
            <input
              type="number"
              min={10}
              max={24}
              value={terminal.fontSize}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10)
                if (v >= 10 && v <= 24) updateTerminalSettings({ fontSize: v })
              }}
              className="w-14 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
            />
          </div>
        </SettingRow>

        {/* Cursor Style */}
        <SettingRow label="Cursor Style">
          <SettingDropdown
            value={terminal.cursorStyle}
            options={[
              { value: 'bar', label: 'Bar' },
              { value: 'block', label: 'Block' },
              { value: 'underline', label: 'Underline' },
            ]}
            onChange={(v) => updateTerminalSettings({ cursorStyle: v as TerminalSettings['cursorStyle'] })}
          />
        </SettingRow>

        {/* Scrollback */}
        <SettingRow label="Scrollback Buffer">
          <input
            type="number"
            min={500}
            max={100000}
            step={500}
            value={terminal.scrollback}
            onChange={(e) => {
              const v = parseInt(e.target.value, 10)
              if (v >= 500 && v <= 100000) updateTerminalSettings({ scrollback: v })
            }}
            className="w-28 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
          />
        </SettingRow>

        {/* Natural Text Editing */}
        <SettingRow label={
          <span title="Opt+Arrow to move by word, Cmd+Arrow for line start/end, Opt+Backspace to delete word">
            Natural Text Editing
          </span>
        }>
          <button
            onClick={() => updateTerminalSettings({ naturalTextEditing: !terminal.naturalTextEditing })}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              terminal.naturalTextEditing ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
                terminal.naturalTextEditing ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </SettingRow>

        {/* Link Click Mode */}
        <SettingRow label={
          <span title="How to activate clickable links (URLs and file paths) in terminal output">
            Link Click Mode
          </span>
        }>
          <SettingDropdown
            value={linkClickMode}
            options={[
              { value: 'click', label: 'Click' },
              { value: 'cmd-click', label: '\u2318 + Click' },
            ]}
            onChange={(v) => setLinkClickMode(v as LinkClickMode)}
          />
        </SettingRow>

        {/* Open Links in Split Pane */}
        <SettingRow label={
          <span title="When split panes are active, open file links in the sibling pane instead of a new tab">
            Open Links in Split Pane
          </span>
        }>
          <button
            onClick={() => setOpenLinksInSplitPane(!openLinksInSplitPane)}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
              openLinksInSplitPane ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-2.5 h-2.5 bg-white block transition-transform ${
                openLinksInSplitPane ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </SettingRow>

      </div>
    </div>
  )
}

// ── Editors & Agents Section ─────────────────────────────────────────
interface EditorDetected {
  id: string
  label: string
  macApp: string
  cliCommand: string
  installed: boolean
  type: 'editor' | 'terminal'
}

interface PresetFormState {
  visible: boolean
  editingId: string | null
  label: string
  command: string
  icon: string
}

function DefaultAgentPickerInline({ presets }: { presets: { id: string; label: string; command: string }[] }): React.JSX.Element {
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const setDefaultAgent = useSettingsStore((s) => s.setDefaultAgent)

  const agentOptions = presets.map((p) => ({
    value: p.command.split(/\s+/)[0],
    label: p.label,
  }))

  return (
    <div className="flex items-center justify-between px-3 py-2.5">
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">Default AI Agent</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Launched with <KeyCombo combo="⇧⌘" />T or from the assistant</div>
      </div>
      <SettingDropdown
        value={defaultAgent}
        options={agentOptions}
        onChange={setDefaultAgent}
      />
    </div>
  )
}

function EditorsAgentsSection(): React.JSX.Element {
  const { presets, fetchPresets } = usePresetsStore()
  const projectSettings = useSettingsStore((s) => s.projectSettings)
  const updateProjectSetting = useSettingsStore((s) => s.updateProjectSetting)
  const fetchSettings = useSettingsStore((s) => s.fetchSettings)
  const [editors, setEditors] = useState<EditorDetected[]>([])
  const [editorsLoading, setEditorsLoading] = useState(false)
  const [presetForm, setPresetForm] = useState<PresetFormState>({
    visible: false,
    editingId: null,
    label: '',
    command: '',
    icon: ''
  })
  const [dragIdx, setDragIdx] = useState<number | null>(null)
  const [dragOverIdx, setDragOverIdx] = useState<number | null>(null)
  const presetDragFromRef = useRef<number | null>(null)
  const presetDropRef = useRef<number | null>(null)
  const formLabelRef = useRef<HTMLInputElement>(null)

  const loadEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<any[]>('projects_get_all_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to load editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  const refreshEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await invoke<any[]>('projects_refresh_editors')
      setEditors(result)
    } catch (err) {
      console.error('Failed to refresh editors:', err)
    } finally {
      setEditorsLoading(false)
    }
  }, [])

  useEffect(() => {
    loadEditors()
    fetchPresets()
  }, [loadEditors, fetchPresets])

  useEffect(() => {
    if (presetForm.visible) {
      requestAnimationFrame(() => formLabelRef.current?.focus())
    }
  }, [presetForm.visible])

  const handleTogglePreset = useCallback(async (id: string, currentEnabled: number) => {
    await invoke('presets_update', { id, enabled: currentEnabled ? 0 : 1 })
    fetchPresets()
  }, [fetchPresets])

  const handleEditPreset = useCallback((preset: typeof presets[number]) => {
    setPresetForm({
      visible: true,
      editingId: preset.id,
      label: preset.label,
      command: preset.command,
      icon: preset.icon ?? ''
    })
  }, [])

  const handleDeletePreset = useCallback(async (id: string) => {
    try {
      await invoke('presets_delete', { id })
      fetchPresets()
    } catch (err) {
      console.error('Failed to delete preset:', err)
    }
  }, [fetchPresets])

  const openAddForm = useCallback(() => {
    setPresetForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
    requestAnimationFrame(() => {
      setPresetForm({ visible: true, editingId: null, label: '', command: '', icon: '' })
    })
  }, [])

  const cancelForm = useCallback(() => {
    setPresetForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const submitForm = useCallback(async () => {
    if (!presetForm.label.trim() || !presetForm.command.trim()) return
    try {
      if (presetForm.editingId) {
        await invoke('presets_update', {
          id: presetForm.editingId,
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || ''
        })
      } else {
        await invoke('presets_create', {
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || undefined
        })
      }
      cancelForm()
      fetchPresets()
    } catch (err) {
      console.error('Failed to save preset:', err)
    }
  }, [presetForm, cancelForm, fetchPresets])

  const handleFormKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      submitForm()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      cancelForm()
    }
  }, [submitForm, cancelForm])

  const handleResetBuiltIns = useCallback(async () => {
    await invoke('presets_reset_built_ins')
    fetchPresets()
  }, [fetchPresets])

  const handlePresetReorderMouseDown = useCallback((e: React.MouseEvent, idx: number) => {
    if (e.button !== 0) return
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && Math.abs(ev.clientY - startY) > 5) {
        started = true
        presetDragFromRef.current = idx
        setDragIdx(idx)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return

      const container = document.querySelector('[data-preset-reorder-container]')
      if (!container) return
      const items = container.querySelectorAll('[data-preset-reorder-index]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientY > rect.top + rect.height / 2) dropIdx = i + 1
      }
      presetDropRef.current = dropIdx
      setDragOverIdx(dropIdx)
    }

    const handleMouseUp = async (): Promise<void> => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started) {
        const fromIdx = presetDragFromRef.current
        const dropIdx = presetDropRef.current
        if (fromIdx !== null && dropIdx !== null && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
          const currentPresets = usePresetsStore.getState().presets
          const sorted = [...currentPresets]
          const [moved] = sorted.splice(fromIdx, 1)
          const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
          sorted.splice(insertAt, 0, moved)
          await invoke('presets_reorder', { ids: sorted.map((p) => p.id) })
          fetchPresets()
        }
      }

      setDragIdx(null)
      setDragOverIdx(null)
      presetDragFromRef.current = null
      presetDropRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [fetchPresets])

  const editorApps = editors.filter((e) => e.type === 'editor')
  const terminalApps = editors.filter((e) => e.type === 'terminal')

  return (
    <div className="max-w-2xl space-y-8">
      {/* ── Defaults ── */}
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Defaults</h2>
        <div className="border border-[var(--color-border)]">
          <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Editor</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Opens files and projects with this editor</div>
            </div>
            <SettingDropdown
              value={(projectSettings['__global__'] as any)?.defaultEditor ?? editorApps.find((e) => e.installed)?.label ?? 'Cursor'}
              options={editorApps.filter((e) => e.installed).map((ed) => ({ value: ed.label, label: ed.label }))}
              onChange={(v) => updateProjectSetting('__global__', 'defaultEditor', v)}
            />
          </div>
          {/* Default Terminal */}
          <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
            <div>
              <div className="text-xs text-[var(--color-text-secondary)]">Default Terminal</div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Right-click a tab to open in this terminal</div>
            </div>
            <SettingDropdown
              value={(projectSettings['__global__'] as any)?.defaultTerminal ?? 'Terminal'}
              options={[
                { value: 'Terminal', label: 'Terminal' },
                ...terminalApps.filter((e) => e.installed).map((ed) => ({ value: ed.label, label: ed.label })),
              ]}
              onChange={(v) => updateProjectSetting('__global__', 'defaultTerminal', v)}
            />
          </div>
          <DefaultAgentPickerInline presets={presets} />
        </div>
      </div>

      {/* ── Editors ── */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Detected Editors</h2>
          <button
            onClick={refreshEditors}
            disabled={editorsLoading}
            className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default font-mono"
          >
            {editorsLoading ? 'Scanning...' : 'Refresh'}
          </button>
        </div>

        <div className="border border-[var(--color-border)]">
          {editorApps.map((editor, i) => (
            <div
              key={editor.id}
              className={`flex items-center gap-3 px-3 py-1.5 ${
                i < editorApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              }`}
            >
              <span
                className={`w-1.5 h-1.5 flex-shrink-0 ${
                  editor.installed ? 'bg-green-500' : 'bg-red-500/60'
                }`}
              />
              <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                {editor.label}
              </span>
              <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                {editor.installed ? editor.cliCommand || editor.macApp : 'not found'}
              </span>
            </div>
          ))}
        </div>

        {/* Terminal apps */}
        {terminalApps.length > 0 && (
          <div className="mt-3">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              Terminal Apps
            </div>
            <div className="border border-[var(--color-border)]">
              {terminalApps.map((app, i) => (
                <div
                  key={app.id}
                  className={`flex items-center gap-3 px-3 py-1.5 ${
                    i < terminalApps.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                  }`}
                >
                  <span
                    className={`w-1.5 h-1.5 flex-shrink-0 ${
                      app.installed ? 'bg-green-500' : 'bg-red-500/60'
                    }`}
                  />
                  <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                    {app.label}
                  </span>
                  <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                    {app.installed ? 'installed' : 'not found'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* ── Agent Presets ── */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Agent Presets</h2>
          <div className="flex items-center gap-2">
            <button
              onClick={handleResetBuiltIns}
              className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
            >
              Reset Built-ins
            </button>
          </div>
        </div>

        <div className="border border-[var(--color-border)]" data-preset-reorder-container>
          {presets.map((preset, i) => (
            <div
              key={preset.id}
              data-preset-reorder-index={i}
              onMouseDown={(e) => handlePresetReorderMouseDown(e, i)}
              className={`relative flex items-center gap-2 px-3 py-1.5 group transition-colors select-none ${
                i < presets.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              } ${dragIdx === i ? 'opacity-30' : ''} cursor-grab active:cursor-grabbing`}
            >
              {dragOverIdx === i && <div className="absolute left-0 right-0 top-0 h-[2px] bg-[var(--color-accent)] z-10" />}
              {dragOverIdx === presets.length && i === presets.length - 1 && <div className="absolute left-0 right-0 bottom-0 h-[2px] bg-[var(--color-accent)] z-10" />}

              {/* Icon — emoji override, otherwise custom drawn icon */}
              <span className="w-5 flex items-center justify-center flex-shrink-0">
                {preset.icon ? (
                  <span className="text-sm leading-none">{preset.icon}</span>
                ) : (
                  <AgentIcon agent={preset.label} size={16} />
                )}
              </span>

              {/* Label */}
              <span className="text-xs text-[var(--color-text-primary)] font-mono w-28 truncate flex-shrink-0">
                {preset.label}
              </span>

              {/* Command */}
              <span className="text-[10px] text-[var(--color-text-muted)] font-mono flex-1 truncate">
                {preset.command}
              </span>

              {/* Built-in badge */}
              {preset.isBuiltIn ? (
                <span className="text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] px-1 py-0.5 flex-shrink-0 font-mono">
                  built-in
                </span>
              ) : null}

              {/* Edit button */}
              <button
                onClick={(e) => { e.stopPropagation(); handleEditPreset(preset) }}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
              >
                edit
              </button>

              {/* Delete button (custom only) */}
              {!preset.isBuiltIn && (
                <button
                  onClick={(e) => { e.stopPropagation(); handleDeletePreset(preset.id) }}
                  className="text-[10px] text-red-400/60 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity no-drag cursor-pointer font-mono flex-shrink-0"
                >
                  del
                </button>
              )}

              {/* Enabled toggle */}
              <button
                onClick={(e) => { e.stopPropagation(); handleTogglePreset(preset.id, preset.enabled) }}
                className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                  preset.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-2.5 h-2.5 bg-white block transition-transform ${
                    preset.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>
          ))}
        </div>

        {/* Inline edit/add form */}
        {presetForm.visible && (
          <div
            className="mt-2 border border-[var(--color-border)] bg-[var(--color-bg-surface)] p-3 space-y-2"
            onKeyDown={handleFormKeyDown}
          >
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
              {presetForm.editingId ? 'Edit Preset' : 'New Preset'}
            </div>
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={presetForm.icon}
                onChange={(e) => setPresetForm((s) => ({ ...s, icon: e.target.value }))}
                placeholder="Icon"
                className="w-10 px-1 py-1 text-center text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
              <input
                ref={formLabelRef}
                type="text"
                value={presetForm.label}
                onChange={(e) => setPresetForm((s) => ({ ...s, label: e.target.value }))}
                placeholder="Label"
                className="w-28 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
              <input
                type="text"
                value={presetForm.command}
                onChange={(e) => setPresetForm((s) => ({ ...s, command: e.target.value }))}
                placeholder="Command (e.g. aider --model gpt-4)"
                className="flex-1 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] no-drag font-mono"
              />
            </div>
            <div className="flex items-center gap-2 justify-end">
              <button
                onClick={cancelForm}
                className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer font-mono"
              >
                Cancel
              </button>
              <button
                onClick={submitForm}
                className="px-3 py-1 text-xs bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/80 no-drag cursor-pointer font-mono"
              >
                {presetForm.editingId ? 'Save' : 'Add'}
              </button>
            </div>
          </div>
        )}

        {/* Add custom preset button */}
        <div className="mt-2">
          <button
            onClick={openAddForm}
            className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono"
          >
            + Add Custom Preset
          </button>
        </div>
      </div>

      {/* ── CLI Install Guide ── */}
      <CLIInstallGuide />
    </div>
  )
}

function CodeEditorSettingsSection(): React.JSX.Element {
  const editor = useSettingsStore((s) => s.editor)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const customThemes = useCustomThemesStore((s) => s.customThemes)
  const creatorOpen = useCustomThemesStore((s) => s.creatorOpen)
  const openCreator = useCustomThemesStore((s) => s.openCreator)
  const closeCreatorStore = useCustomThemesStore((s) => s.closeCreator)
  const [showCreator, setShowCreator] = useState(false)
  const [editingThemePath, setEditingThemePath] = useState<string | undefined>(undefined)
  const [showThemeManager, setShowThemeManager] = useState(false)
  const isCustomTheme = editor.theme.startsWith('custom:')
  const deleteCustomTheme = useCustomThemesStore((s) => s.deleteCustomTheme)

  const LIGATURE_FONTS = new Set(['Fira Code', 'JetBrains Mono', 'Lilex'])
  const fontSupportsLigatures = LIGATURE_FONTS.has(editor.fontFamily)

  // Build combined theme list: built-in + custom
  const allThemeOptions = useMemo(() => {
    const builtIn = EDITOR_THEMES.map(t => ({ value: t.id, label: t.label }))
    const custom = customThemes.map(t => ({ value: t.id, label: `${t.name}` }))
    if (custom.length > 0) {
      return [...builtIn, { value: '__divider__', label: '── Custom ──' }, ...custom]
    }
    return builtIn
  }, [customThemes])

  // Open creator for an existing custom theme
  const handleCustomize = useCallback(() => {
    const theme = customThemes.find((t) => t.id === editor.theme)
    setEditingThemePath(theme?.path)
    setShowCreator(true)
  }, [customThemes, editor.theme])

  // Open creator for a brand new theme
  const handleNewTheme = useCallback(() => {
    setEditingThemePath(undefined)
    setShowCreator(true)
  }, [])

  const toggleRow = (label: string, description: string, key: keyof typeof editor, isLast = false, disabled = false) => (
    <div key={key} className={`flex items-center justify-between px-3 py-2.5 ${!isLast ? 'border-b border-[var(--color-border)]' : ''} ${disabled ? 'opacity-40' : ''}`}>
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">{label}</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{description}</div>
      </div>
      <button
        onClick={() => { if (!disabled) updateEditorSettings({ [key]: !editor[key] }) }}
        className={`w-7 h-3.5 flex items-center transition-colors flex-shrink-0 ${disabled ? 'cursor-default' : 'no-drag cursor-pointer'} ${
          editor[key] && !disabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
      >
        <span className={`w-2.5 h-2.5 bg-white block transition-transform ${
          editor[key] && !disabled ? 'translate-x-3.5' : 'translate-x-0.5'
        }`} />
      </button>
    </div>
  )

  const dropdownRow = (label: string, description: string, value: string, options: { value: string; label: string }[], onChange: (v: string) => void, isLast = false) => (
    <div className={`flex items-center justify-between px-3 py-2.5 ${!isLast ? 'border-b border-[var(--color-border)]' : ''}`}>
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">{label}</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{description}</div>
      </div>
      <SettingDropdown value={value} options={options} onChange={onChange} />
    </div>
  )

  // Sample code for the live preview — K2SO coding in a galaxy far, far away
  const previewCode = `import { useState, useEffect, useCallback } from 'react'

// K2SO Security Droid — Imperial Data Vault Access Module
// "I find that answer vague and unconvincing." — K2SO

interface SecurityProtocol {
  clearanceLevel: 'rebel' | 'imperial' | 'classified'
  accessCode: string
  probabilityOfSuccess: number
  isStealthMode?: boolean
}

type MissionStatus = 'infiltrating' | 'compromised' | 'success' | 'told-you-so'

/**
 * Calculates the survival odds for a given mission.
 * Spoiler: they're never good enough for K2SO's standards.
 */
export function calculateSurvivalOdds(
  crew: string[],
  hasForceSensitive: boolean,
  imperialPresence: number
): { odds: number; commentary: string } {
  const baseOdds = 100 - (imperialPresence * 12.7)
  const crewBonus = crew.length * 3.2
  const forceMultiplier = hasForceSensitive ? 1.47 : 0.89

  const finalOdds = Math.min(
    97.6,
    Math.max(0, (baseOdds + crewBonus) * forceMultiplier)
  )

  // K2SO always has something to say about the odds
  const commentary =
    finalOdds > 80 ? "Acceptable. I still don't like it." :
    finalOdds > 50 ? "I have a bad feeling about this." :
    finalOdds > 20 ? "Would you like to know the probability of failure?" :
    "I'm not very optimistic about our chances."

  return { odds: Math.round(finalOdds * 100) / 100, commentary }
}

export function useImperialVault(protocol: SecurityProtocol) {
  const [status, setStatus] = useState<MissionStatus>('infiltrating')
  const [dataStolen, setDataStolen] = useState<string[]>([])
  const [alarmTriggered, setAlarmTriggered] = useState(false)

  // Attempt to slice into the Imperial network
  const sliceTerminal = useCallback(async (terminalId: string) => {
    if (protocol.probabilityOfSuccess < 32.5) {
      setStatus('told-you-so')
      return { success: false, message: "I told you this would happen." }
    }

    try {
      const deathStarPlans = await fetchClassifiedData(terminalId)
      setDataStolen((prev) => [...prev, ...deathStarPlans])
      setStatus('success')
      return { success: true, message: "The plans are in the droid." }
    } catch {
      setAlarmTriggered(true)
      setStatus('compromised')
      return { success: false, message: "There are a lot of them." }
    }
  }, [protocol.probabilityOfSuccess])

  // Monitor for Stormtroopers (they never check behind crates)
  useEffect(() => {
    if (!protocol.isStealthMode) return

    const patrol = setInterval(() => {
      const detected = Math.random() > 0.85
      if (detected && !alarmTriggered) {
        setAlarmTriggered(true)
        setStatus('compromised')
        console.warn('[K2SO] Congratulations. You are being rescued.')
      }
    }, 5000)

    return () => clearInterval(patrol)
  }, [protocol.isStealthMode, alarmTriggered])

  return { status, dataStolen, alarmTriggered, sliceTerminal }
}

async function fetchClassifiedData(id: string): Promise<string[]> {
  // "Quiet! And there is a fresh one if you mouth off again."
  const response = await fetch(\`/api/imperial/\${id}/plans\`)
  if (!response.ok) throw new Error('Access denied. Probably.')
  return response.json()
}
`

  // Demo diff data: K2SO's latest code review changes
  const demoChanges = useMemo(() => {
    const m = new Map<number, 'added' | 'modified' | 'deleted'>()
    // Added stealth mode to the protocol
    m.set(11, 'added')
    // New mission status type
    m.set(14, 'modified')
    // Added survival odds calculator
    m.set(21, 'added')
    m.set(22, 'added')
    m.set(23, 'added')
    m.set(24, 'added')
    m.set(25, 'added')
    // Modified odds calculation
    m.set(30, 'modified')
    m.set(31, 'modified')
    // K2SO commentary — added
    m.set(37, 'added')
    m.set(38, 'added')
    m.set(39, 'added')
    m.set(40, 'added')
    // Deleted old approach
    m.set(55, 'deleted')
    // Modified stealth monitoring
    m.set(73, 'modified')
    m.set(74, 'modified')
    m.set(75, 'modified')
    m.set(76, 'modified')
    // Added console.warn
    m.set(79, 'added')
    return m
  }, [])

  if (showCreator) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <CustomThemeCreator
            currentThemeId={editor.theme}
            existingThemePath={editingThemePath}
            onClose={() => setShowCreator(false)}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  return (
    <div className="flex gap-6">
      {/* Settings panel */}
      <div className="max-w-xl flex-1 space-y-6 min-w-0">
        {/* ── Appearance ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Appearance</h2>
          <div className="border border-[var(--color-border)]">
            <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
              <div>
                <div className="text-xs text-[var(--color-text-secondary)]">Theme</div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Color theme for the code editor</div>
              </div>
              <div className="flex items-center gap-1.5">
                {customThemes.length > 0 && (
                  <button
                    onClick={() => setShowThemeManager(!showThemeManager)}
                    className="px-2 py-1 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag"
                  >
                    Manage
                  </button>
                )}
                {isCustomTheme && (
                  <button
                    onClick={handleCustomize}
                    className="px-2 py-1 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer no-drag"
                  >
                    Customize
                  </button>
                )}
                <SettingDropdown
                  value={editor.theme}
                  options={allThemeOptions.filter(o => o.value !== '__divider__')}
                  onChange={(v) => updateEditorSettings({ theme: v })}
                />
                <button
                  onClick={handleNewTheme}
                  className="px-2 py-1 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag whitespace-nowrap"
                >
                  + New
                </button>
              </div>
            </div>
            {/* Theme manager — list custom themes with edit/delete */}
            {showThemeManager && customThemes.length > 0 && (
              <div className="border-b border-[var(--color-border)] bg-[var(--color-bg)]/50">
                <div className="px-3 py-2">
                  <div className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider mb-2">Custom Themes</div>
                  {customThemes.map((t) => (
                    <div key={t.id} className="flex items-center justify-between py-1.5 group">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="w-3 h-3 flex-shrink-0 border border-[var(--color-border)]" style={{ backgroundColor: t.colors.bg }} />
                        <span className="text-xs text-[var(--color-text-primary)] truncate">{t.name}</span>
                        {editor.theme === t.id && (
                          <span className="text-[9px] text-[var(--color-accent)] flex-shrink-0">active</span>
                        )}
                      </div>
                      <div className="flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
                        <button
                          onClick={() => {
                            setEditingThemePath(t.path)
                            setShowCreator(true)
                            setShowThemeManager(false)
                          }}
                          className="px-1.5 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag"
                        >
                          Edit
                        </button>
                        <button
                          onClick={async () => {
                            if (editor.theme === t.id) {
                              updateEditorSettings({ theme: 'k2so-dark' })
                            }
                            await deleteCustomTheme(t.id)
                          }}
                          className="px-1.5 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-red-400 cursor-pointer no-drag"
                        >
                          Delete
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {dropdownRow('Font Family', 'Monospace font for code editing', editor.fontFamily,
              EDITOR_FONTS.map(f => ({ value: f.id, label: f.label })),
              (v) => updateEditorSettings({ fontFamily: v })
            )}
            {dropdownRow('Font Size', 'Editor text size in pixels', String(editor.fontSize),
              [10, 11, 12, 13, 14, 15, 16, 18, 20].map(n => ({ value: String(n), label: `${n}px` })),
              (v) => updateEditorSettings({ fontSize: Number(v) })
            )}
            {toggleRow('Font Ligatures', fontSupportsLigatures ? 'Enable programming ligatures (e.g. => becomes arrow)' : 'Requires Fira Code, JetBrains Mono, or Lilex', 'fontLigatures', false, !fontSupportsLigatures)}
            {dropdownRow('Cursor Style', 'Shape of the text cursor', editor.cursorStyle,
              [{ value: 'bar', label: 'Bar' }, { value: 'block', label: 'Block' }, { value: 'underline', label: 'Underline' }],
              (v) => updateEditorSettings({ cursorStyle: v as 'bar' | 'block' | 'underline' })
            )}
            {toggleRow('Cursor Blink', 'Animate the cursor blinking', 'cursorBlink', true)}
          </div>
        </div>

        {/* ── Editing ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Editing</h2>
          <div className="border border-[var(--color-border)]">
            {dropdownRow('Tab Size', 'Default spaces per indentation (languages may override)', String(editor.tabSize),
              [{ value: '2', label: '2' }, { value: '4', label: '4' }, { value: '8', label: '8' }],
              (v) => updateEditorSettings({ tabSize: Number(v) })
            )}
            {toggleRow('Word Wrap', 'Wrap long lines instead of horizontal scrolling', 'wordWrap')}
            {toggleRow('Autocomplete', 'Show word-based completion suggestions as you type', 'autocomplete')}
            {toggleRow('Bracket Matching', 'Highlight matching brackets', 'bracketMatching')}
            {toggleRow('Format on Save', 'Auto-format with Prettier, rustfmt, or black on Cmd+S', 'formatOnSave')}
            {toggleRow('Show Whitespace', 'Render spaces and tabs as visible dots', 'showWhitespace', true)}
          </div>
        </div>

        {/* ── Gutter & Display ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Gutter & Display</h2>
          <div className="border border-[var(--color-border)]">
            {toggleRow('Line Numbers', 'Show line numbers in the gutter', 'lineNumbers')}
            {toggleRow('Indent Guides', 'Show vertical indentation guide lines', 'indentGuides')}
            {toggleRow('Code Folding', 'Show fold/unfold arrows in the gutter', 'foldGutter')}
            {toggleRow('Highlight Active Line', 'Subtle background highlight on the current line', 'highlightActiveLine')}
            {toggleRow('Scroll Past End', 'Allow scrolling beyond the last line', 'scrollPastEnd')}
            {toggleRow('Minimap', 'Show a miniature overview of the file on the right', 'minimap')}
            {dropdownRow('Diff Style', 'How changed lines appear in the editor', editor.diffStyle,
              [{ value: 'gutter', label: 'Gutter' }, { value: 'inline', label: 'Inline (PR view)' }],
              (v) => updateEditorSettings({ diffStyle: v as 'gutter' | 'inline' })
            )}
            {toggleRow('Scrollbar Annotations', 'Show colored markers on the scrollbar where code was changed', 'scrollbarAnnotations')}
            {toggleRow('Sticky Scroll', 'Pin current function/class header at the top', 'stickyScroll', true)}
          </div>
        </div>

        {/* ── Keybindings & Modes ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Keybindings & Modes</h2>
          <div className="border border-[var(--color-border)]">
            {toggleRow('Vim Mode', 'Full vim keybinding emulation (hjkl, modes, commands)', 'vimMode')}
            <div className="px-3 py-2.5 border-b border-[var(--color-border)]">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Select Next Occurrence</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+D</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Add a cursor at the next match of the selected word</div>
            </div>
            <div className="px-3 py-2.5 border-b border-[var(--color-border)]">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Find & Replace</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+F / Cmd+H</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Search with regex, case-sensitive, and replace support</div>
            </div>
            <div className="px-3 py-2.5">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Fold / Unfold</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+Shift+[ / ]</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Collapse or expand code blocks at the cursor</div>
            </div>
          </div>
        </div>
      </div>

      {/* ── Live Preview (sticky) ── */}
      <div className="flex-1 min-w-[400px] sticky top-0 self-start">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Preview</h2>
        <div className="border border-[var(--color-border)] h-[calc(100vh-120px)] overflow-hidden">
          <CodeEditor
            code={previewCode}
            filePath="preview.tsx"
            onSave={() => {}}
            onChange={() => {}}
            readOnly
            demoLineChanges={demoChanges}
          />
        </div>
      </div>
    </div>
  )
}

// ── CLI Install Guide Data ──────────────────────────────────────────
const CLI_INSTALL_ENTRIES: {
  name: string
  command: string
  installCommand: string
  docs: string
  notes?: string
}[] = [
  {
    name: 'Claude Code',
    command: 'claude',
    installCommand: 'npm install -g @anthropic-ai/claude-code',
    docs: 'https://docs.anthropic.com/en/docs/claude-code',
    notes: 'Requires Node.js 18+. After install, run "claude" to authenticate with your Anthropic account.'
  },
  {
    name: 'OpenAI Codex',
    command: 'codex',
    installCommand: 'npm install -g @openai/codex',
    docs: 'https://github.com/openai/codex',
    notes: 'Requires Node.js 22+. After install, set your OPENAI_API_KEY or log in via "codex --login".'
  },
  {
    name: 'Gemini CLI',
    command: 'gemini',
    installCommand: 'npm install -g @anthropic-ai/gemini-cli',
    docs: 'https://geminicli.com',
    notes: 'Requires Node.js 18+. Authenticate with your Google account on first run.'
  },
  {
    name: 'GitHub Copilot CLI',
    command: 'copilot',
    installCommand: 'npm install -g @anthropic-ai/copilot-cli',
    docs: 'https://docs.github.com/en/copilot/how-tos/copilot-cli',
    notes: 'Requires an active GitHub Copilot subscription. Authenticate with "gh auth login" first.'
  },
  {
    name: 'Aider',
    command: 'aider',
    installCommand: 'pip install aider-chat',
    docs: 'https://aider.chat/docs/install.html',
    notes: 'Requires Python 3.9+. Configure your API key for the model provider you want to use.'
  },
  {
    name: 'Cursor Agent',
    command: 'cursor-agent',
    installCommand: 'npm install -g cursor-agent',
    docs: 'https://docs.cursor.com',
    notes: 'The standalone CLI agent from Cursor. Requires a Cursor subscription.'
  },
  {
    name: 'OpenCode',
    command: 'opencode',
    installCommand: 'curl -fsSL https://opencode.ai/install | bash',
    docs: 'https://opencode.ai',
    notes: 'A terminal-based AI coding assistant. Supports multiple model providers.'
  },
  {
    name: 'Code Puppy',
    command: 'codepuppy',
    installCommand: 'npm install -g codepuppy',
    docs: 'https://codepuppy.ai',
    notes: 'Lightweight AI coding assistant for the terminal.'
  },
  {
    name: 'Goose',
    command: 'goose',
    installCommand: 'curl -fsSL https://github.com/block/goose/releases/latest/download/install.sh | bash',
    docs: 'https://github.com/block/goose',
    notes: 'An open-source AI developer agent from Block. Supports multiple model providers.'
  },
  {
    name: 'Pi',
    command: 'pi',
    installCommand: 'npm install -g @mariozechner/pi-coding-agent',
    docs: 'https://github.com/badlogic/pi-mono',
    notes: 'Minimal coding agent with 15+ LLM providers. Supports OAuth login (/login) for Claude, Copilot, Gemini subscriptions, or use API keys directly.'
  },
  {
    name: 'Ollama',
    command: 'ollama',
    installCommand: 'curl -fsSL https://ollama.ai/install.sh | sh',
    docs: 'https://ollama.ai',
    notes: 'Run large language models locally. After install, pull a model with "ollama pull llama3".'
  }
]

function CLIVersionRow(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    installedVersion: string | null
    bundledVersion: string | null
    updateAvailable: boolean
  } | null>(null)
  const [loading, setLoading] = useState(false)
  const [checking, setChecking] = useState(false)

  const checkStatus = useCallback(async () => {
    try {
      const result = await invoke<{
        installed: boolean
        installedVersion: string | null
        bundledVersion: string | null
        updateAvailable: boolean
      }>('cli_install_status')
      setStatus(result)
    } catch {
      // silently fail
    }
  }, [])

  useEffect(() => { checkStatus() }, [checkStatus])

  const handleInstallOrUpdate = useCallback(async () => {
    setLoading(true)
    try {
      await invoke('cli_install')
      await checkStatus()
    } catch (err) {
      console.error('[cli]', err)
    } finally {
      setLoading(false)
    }
  }, [checkStatus])

  const handleCheckForUpdates = useCallback(async () => {
    setChecking(true)
    try {
      await checkStatus()
    } finally {
      setChecking(false)
    }
  }, [checkStatus])

  // Compare versions properly — only show update if bundled is actually newer
  const compareVersions = (a: string, b: string): number => {
    const pa = a.split('.').map(Number)
    const pb = b.split('.').map(Number)
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const va = pa[i] || 0
      const vb = pb[i] || 0
      if (va > vb) return 1
      if (va < vb) return -1
    }
    return 0
  }
  const updateAvailable = status?.installed && status.bundledVersion && status.installedVersion
    && compareVersions(status.bundledVersion, status.installedVersion) > 0

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">CLI Version</span>
      <div className="flex items-center gap-3">
        {status?.installed ? (
          <>
            <div className="flex items-center gap-1.5">
              <span
                className="w-1.5 h-1.5 flex-shrink-0"
                style={{ backgroundColor: updateAvailable ? '#eab308' : '#4ade80' }}
              />
              <span className="text-xs text-[var(--color-text-muted)]">
                v{status.installedVersion || '?'}
              </span>
            </div>
            {updateAvailable ? (
              <button
                onClick={handleInstallOrUpdate}
                disabled={loading}
                className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Updating...' : `Update to v${status.bundledVersion}`}
              </button>
            ) : (
              <button
                onClick={handleCheckForUpdates}
                disabled={checking}
                className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                {checking ? 'Checking...' : 'Check for Updates'}
              </button>
            )}
          </>
        ) : (
          <>
            <span className="text-xs text-[var(--color-text-muted)]">Not installed</span>
            <button
              onClick={handleInstallOrUpdate}
              disabled={loading}
              className="px-2 py-0.5 text-[10px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
            >
              {loading ? 'Installing...' : 'Install'}
            </button>
          </>
        )}
      </div>
    </div>
  )
}

function K2SOCLIInstall(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    symlinkPath: string
    target: string | null
    bundledPath: string | null
    bundledVersion: string | null
    installedVersion: string | null
    updateAvailable: boolean
  } | null>(null)
  const [loading, setLoading] = useState(false)

  const checkStatus = useCallback(async () => {
    try {
      const result = await invoke<{
        installed: boolean
        symlinkPath: string
        target: string | null
        bundledPath: string | null
        bundledVersion: string | null
        installedVersion: string | null
        updateAvailable: boolean
      }>('cli_install_status')
      setStatus(result)
    } catch {
      // silently fail
    }
  }, [])

  useEffect(() => { checkStatus() }, [checkStatus])

  const handleInstall = useCallback(async () => {
    setLoading(true)
    try {
      await invoke('cli_install')
      await checkStatus()
    } catch (err) {
      console.error('[cli] install failed:', err)
    } finally {
      setLoading(false)
    }
  }, [checkStatus])

  const handleUninstall = useCallback(async () => {
    setLoading(true)
    try {
      await invoke('cli_uninstall')
      await checkStatus()
    } catch (err) {
      console.error('[cli] uninstall failed:', err)
    } finally {
      setLoading(false)
    }
  }, [checkStatus])

  return (
    <div className="border border-[var(--color-border)]">
      <div className="flex items-center justify-between px-4 py-3">
        <div>
          <span className="text-xs text-[var(--color-text-primary)]">Install CLI</span>
          <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
            Add <span className="font-mono text-[var(--color-text-secondary)]">k2so</span> to your PATH for use in any terminal
          </p>
        </div>
        <div className="flex items-center gap-3 flex-shrink-0">
          {status?.installed ? (
            <>
              <div className="flex items-center gap-2">
                <div className={`w-2 h-2 flex-shrink-0 ${status.updateAvailable ? 'bg-yellow-500' : 'bg-green-500'}`} />
                <span className="text-xs text-[var(--color-text-secondary)]">
                  {status.installedVersion ? `v${status.installedVersion}` : 'Installed'}
                </span>
              </div>
              {status.updateAvailable ? (
                <button
                  onClick={handleInstall}
                  disabled={loading}
                  className="px-3 py-1.5 text-[11px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
                >
                  {loading ? 'Updating...' : `Update to v${status.bundledVersion}`}
                </button>
              ) : (
                <button
                  onClick={handleUninstall}
                  disabled={loading}
                  className="px-3 py-1.5 text-[11px] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer disabled:opacity-50"
                >
                  {loading ? 'Removing...' : 'Uninstall'}
                </button>
              )}
            </>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <div className="w-2 h-2 bg-[var(--color-text-muted)] opacity-40 flex-shrink-0" />
                <span className="text-xs text-[var(--color-text-muted)]">Not installed</span>
              </div>
              <button
                onClick={handleInstall}
                disabled={loading}
                className="px-3 py-1.5 text-[11px] bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Installing...' : 'Install k2so to PATH'}
              </button>
            </>
          )}
        </div>
      </div>
      {status?.installed && status?.target && (
        <div className="px-4 pb-3">
          <p className="text-[10px] text-[var(--color-text-muted)] font-mono">
            {status.symlinkPath} → {status.target}
          </p>
        </div>
      )}
    </div>
  )
}

function CLIInstallGuide(): React.JSX.Element {
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null)
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null)

  const handleCopy = useCallback((installCommand: string, idx: number) => {
    navigator.clipboard.writeText(installCommand)
    setCopiedIdx(idx)
    setTimeout(() => setCopiedIdx(null), 2000)
  }, [])

  return (
    <div>
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">CLI Tools Setup</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
        Install instructions for each AI coding agent. Click to expand.
      </p>

      <div className="border border-[var(--color-border)]">
        {CLI_INSTALL_ENTRIES.map((entry, i) => {
          const isExpanded = expandedIdx === i
          const isCopied = copiedIdx === i

          return (
            <div
              key={entry.command}
              className={i < CLI_INSTALL_ENTRIES.length - 1 ? 'border-b border-[var(--color-border)]' : ''}
            >
              {/* Header row — clickable to expand */}
              <button
                className="w-full flex items-center gap-3 px-3 py-2 text-left hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer"
                onClick={() => setExpandedIdx(isExpanded ? null : i)}
              >
                <svg
                  width="8"
                  height="8"
                  viewBox="0 0 8 8"
                  fill="currentColor"
                  className={`flex-shrink-0 text-[var(--color-text-muted)] transition-transform ${isExpanded ? 'rotate-90' : ''}`}
                >
                  <polygon points="1,0 7,4 1,8" />
                </svg>
                <span className="text-xs text-[var(--color-text-primary)] font-mono flex-1">
                  {entry.name}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono">
                  {entry.command}
                </span>
              </button>

              {/* Expanded detail */}
              {isExpanded && (
                <div className="px-3 pb-3 pt-0 ml-5 space-y-2">
                  {/* Install command with copy button */}
                  <div className="flex items-center gap-2">
                    <code className="flex-1 text-[11px] font-mono bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1.5 text-[var(--color-text-primary)] select-all">
                      {entry.installCommand}
                    </code>
                    <button
                      onClick={() => handleCopy(entry.installCommand, i)}
                      className="flex-shrink-0 px-2 py-1.5 text-[10px] font-mono border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer"
                      style={{ color: isCopied ? '#22c55e' : 'var(--color-text-muted)' }}
                    >
                      {isCopied ? 'Copied!' : 'Copy'}
                    </button>
                  </div>

                  {/* Notes */}
                  {entry.notes && (
                    <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                      {entry.notes}
                    </p>
                  )}

                  {/* Docs link */}
                  <a
                    href={entry.docs}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="inline-block text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 font-mono transition-colors"
                  >
                    Documentation →
                  </a>
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Keybindings Section ──────────────────────────────────────────────
function KeybindingsSection(): React.JSX.Element {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const updateKeybinding = useSettingsStore((s) => s.updateKeybinding)
  const resetKeybinding = useSettingsStore((s) => s.resetKeybinding)
  const resetAllKeybindings = useSettingsStore((s) => s.resetAllKeybindings)
  const shortcutLayout = useTerminalSettingsStore((s) => s.shortcutLayout)
  const setShortcutLayout = useTerminalSettingsStore((s) => s.setShortcutLayout)
  const [capturing, setCapturing] = useState<string | null>(null)

  // Build a map of combo -> ids to detect conflicts
  const comboToIds: Record<string, string[]> = {}
  for (const h of HOTKEYS) {
    const combo = getEffectiveKeybinding(keybindings, h.id)
    if (!comboToIds[combo]) comboToIds[combo] = []
    comboToIds[combo].push(h.id)
  }

  const hasConflict = (id: string): boolean => {
    const combo = getEffectiveKeybinding(keybindings, id)
    return (comboToIds[combo]?.length ?? 0) > 1
  }

  // Group hotkeys by category
  const categories = ['Terminal', 'Tabs', 'Navigation', 'App'] as const
  const grouped = categories.map((cat) => ({
    category: cat,
    items: HOTKEYS.filter((h) => h.category === cat)
  }))

  return (
    <div className="max-w-2xl">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Keybindings</h2>
        <button
          onClick={resetAllKeybindings}
          className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
        >
          Reset All to Defaults
        </button>
      </div>

      <div className="text-xs text-[var(--color-text-muted)] mb-3">
        Click a binding to rebind. Press Escape to cancel. Reserved keys: {RESERVED_KEYS.join(', ')}
      </div>

      {/* Workspace number shortcut layout */}
      <div className="mb-4 border border-[var(--color-border)] px-3 py-2.5 flex items-center justify-between">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Workspace Number Shortcuts</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            {shortcutLayout === 'cmd-active-cmdshift-pinned'
              ? <><KeyCombo combo="⌘" /> 1-9 switches Active, <KeyCombo combo="⇧⌘" /> 1-9 switches Pinned</>
              : <><KeyCombo combo="⌘" /> 1-9 switches Pinned, <KeyCombo combo="⇧⌘" /> 1-9 switches Active</>
            }
          </div>
        </div>
        <button
          onClick={() => setShortcutLayout(
            shortcutLayout === 'cmd-active-cmdshift-pinned'
              ? 'cmd-pinned-cmdshift-active'
              : 'cmd-active-cmdshift-pinned'
          )}
          className="px-3 py-1 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] no-drag cursor-pointer font-mono flex-shrink-0"
        >
          Swap
        </button>
      </div>

      <div className="space-y-4">
        {grouped.map(({ category, items }) => (
          <div key={category}>
            <div className="text-xs font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              {category}
            </div>
            {category === 'Navigation' && (
              <div className="border border-[var(--color-border)] mb-px">
                <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)]">
                  <div>
                    <span className="text-xs text-[var(--color-text-secondary)]">Active Workspaces</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">1-9</span>
                  </div>
                  <span className="text-xs font-mono text-[var(--color-text-muted)] bg-white/[0.06] px-2 py-0.5">
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌘ 1-9' : '⌥⌘ 1-9'} />
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <div>
                    <span className="text-xs text-[var(--color-text-secondary)]">Pinned Workspaces</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">1-9</span>
                  </div>
                  <span className="text-xs font-mono text-[var(--color-text-muted)] bg-white/[0.06] px-2 py-0.5">
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌥⌘ 1-9' : '⌘ 1-9'} />
                  </span>
                </div>
              </div>
            )}
            <div className="border border-[var(--color-border)]">
              {items.map((hotkey, i) => (
                <KeybindingRow
                  key={hotkey.id}
                  hotkey={hotkey}
                  combo={getEffectiveKeybinding(keybindings, hotkey.id)}
                  isCustom={!!keybindings[hotkey.id]}
                  conflict={hasConflict(hotkey.id)}
                  capturing={capturing === hotkey.id}
                  onStartCapture={() => setCapturing(hotkey.id)}
                  onCapture={(combo) => {
                    updateKeybinding(hotkey.id, combo)
                    setCapturing(null)
                  }}
                  onCancelCapture={() => setCapturing(null)}
                  onReset={() => resetKeybinding(hotkey.id)}
                  hasBorder={i < items.length - 1}
                />
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}

interface KeybindingRowProps {
  hotkey: HotkeyDefinition
  combo: string
  isCustom: boolean
  conflict: boolean
  capturing: boolean
  onStartCapture: () => void
  onCapture: (combo: string) => void
  onCancelCapture: () => void
  onReset: () => void
  hasBorder: boolean
}

function KeybindingRow({
  hotkey,
  combo,
  isCustom,
  conflict,
  capturing,
  onStartCapture,
  onCapture,
  onCancelCapture,
  onReset,
  hasBorder
}: KeybindingRowProps): React.JSX.Element {
  const captureRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!capturing) return

    const handler = (e: KeyboardEvent): void => {
      e.preventDefault()
      e.stopPropagation()

      if (e.key === 'Escape') {
        onCancelCapture()
        return
      }

      const newCombo = keyEventToCombo(e)
      // Ignore bare modifier presses
      if (['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) return

      if (isReservedKey(newCombo)) return

      onCapture(newCombo)
    }

    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [capturing, onCapture, onCancelCapture])

  useEffect(() => {
    if (capturing && captureRef.current) {
      captureRef.current.focus()
    }
  }, [capturing])

  return (
    <div
      className={`flex items-center justify-between px-3 py-1.5 ${
        hasBorder ? 'border-b border-[var(--color-border)]' : ''
      } ${conflict ? 'bg-red-500/10' : ''}`}
    >
      <span className="text-xs text-[var(--color-text-secondary)] flex-1">{hotkey.label}</span>

      <div className="flex items-center gap-2">
        {capturing ? (
          <button
            ref={captureRef}
            className="px-2 py-0.5 text-xs bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/50 text-[var(--color-accent)] no-drag cursor-pointer animate-pulse"
          >
            Press a key...
          </button>
        ) : (
          <button
            onClick={onStartCapture}
            className={`px-2 py-0.5 text-xs border no-drag cursor-pointer ${
              conflict
                ? 'bg-red-500/20 border-red-500/40 text-red-400'
                : 'bg-[var(--color-bg-surface)] border-[var(--color-border)] text-[var(--color-text-primary)]'
            } hover:border-[var(--color-text-muted)]`}
          >
            <KeyCombo combo={formatKeyCombo(combo)} />
          </button>
        )}

        {isCustom && (
          <button
            onClick={onReset}
            className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            title="Reset to default"
          >
            x
          </button>
        )}
      </div>
    </div>
  )
}

// ── Workspaces Section — Master-Detail with Focus Group Folders ────────
// ── Timer Section ────────────────────────────────────────────────────

const BUILT_IN_THEMES = [
  { value: 'rocket', label: 'Rocket Launch' },
  { value: 'matrix', label: 'Matrix Rain' },
  { value: 'retro', label: 'Retro Arcade' },
]

// Common IANA timezones for the dropdown
const COMMON_TIMEZONES = [
  '',
  'UTC',
  'America/New_York',
  'America/Chicago',
  'America/Denver',
  'America/Los_Angeles',
  'America/Anchorage',
  'Pacific/Honolulu',
  'America/Toronto',
  'America/Vancouver',
  'America/Sao_Paulo',
  'America/Argentina/Buenos_Aires',
  'Europe/London',
  'Europe/Paris',
  'Europe/Berlin',
  'Europe/Moscow',
  'Asia/Dubai',
  'Asia/Kolkata',
  'Asia/Singapore',
  'Asia/Shanghai',
  'Asia/Tokyo',
  'Asia/Seoul',
  'Australia/Sydney',
  'Australia/Melbourne',
  'Pacific/Auckland',
]

function TimerSection(): React.JSX.Element {
  const visible = useTimerStore((s) => s.visible)
  const countdownEnabled = useTimerStore((s) => s.countdownEnabled)
  const countdownTheme = useTimerStore((s) => s.countdownTheme)
  const skipMemo = useTimerStore((s) => s.skipMemo)
  const timezone = useTimerStore((s) => s.timezone)
  const customThemes = useTimerStore((s) => s.customThemes)
  const entries = useTimerStore((s) => s.entries)
  const fetchEntries = useTimerStore((s) => s.fetchEntries)
  const deleteEntry = useTimerStore((s) => s.deleteEntry)
  const exportEntries = useTimerStore((s) => s.exportEntries)
  const updateTimerSetting = useTimerStore((s) => s.updateTimerSetting)

  const projects = useProjectsStore((s) => s.projects)

  // Filter state
  const [filterStart, setFilterStart] = useState('')
  const [filterEnd, setFilterEnd] = useState('')
  const [filterProject, setFilterProject] = useState('')

  // Load entries on mount
  useEffect(() => {
    fetchEntries()
  }, [fetchEntries])

  const handleFilter = useCallback(() => {
    const start = filterStart ? Math.floor(new Date(filterStart).getTime() / 1000) : undefined
    const end = filterEnd ? Math.floor(new Date(filterEnd + 'T23:59:59').getTime() / 1000) : undefined
    fetchEntries(start, end, filterProject || undefined)
  }, [filterStart, filterEnd, filterProject, fetchEntries])

  // Re-fetch when filters change
  useEffect(() => {
    handleFilter()
  }, [handleFilter])

  const handleExport = useCallback(async (format: 'csv' | 'json') => {
    const start = filterStart ? Math.floor(new Date(filterStart).getTime() / 1000) : undefined
    const end = filterEnd ? Math.floor(new Date(filterEnd + 'T23:59:59').getTime() / 1000) : undefined
    const data = await exportEntries(format, start, end, filterProject || undefined)
    if (!data) return

    // Download as file
    const blob = new Blob([data], { type: format === 'csv' ? 'text/csv' : 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `k2so-time-entries.${format}`
    a.click()
    URL.revokeObjectURL(url)
  }, [filterStart, filterEnd, filterProject, exportEntries])

  const themeInputRef = useRef<HTMLInputElement>(null)

  const handleUploadTheme = useCallback(() => {
    themeInputRef.current?.click()
  }, [])

  const handleThemeFileChange = useCallback(async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (!file) return
    // Reset input so the same file can be re-selected
    e.target.value = ''

    try {
      const text = await file.text()
      const parsed = JSON.parse(text) as CountdownThemeConfig

      // Basic validation
      if (!parsed.name || !parsed.backgroundColor || !parsed.textColor || !parsed.finalText) {
        console.error('[timer] Invalid theme: must have name, backgroundColor, textColor, and finalText')
        return
      }
      if (!parsed.countdownTexts || !Array.isArray(parsed.countdownTexts)) {
        parsed.countdownTexts = ['3', '2', '1']
      }
      if (!parsed.animationPreset) {
        parsed.animationPreset = 'fade'
      }
      if (!parsed.flowTitles || !Array.isArray(parsed.flowTitles)) {
        parsed.flowTitles = []
      }

      const updated = [...customThemes, parsed]
      await updateTimerSetting('customThemes', updated)
    } catch (err) {
      console.error('[timer] Failed to upload theme:', err)
    }
  }, [customThemes, updateTimerSetting])

  const handleDownloadReference = useCallback(() => {
    const reference: CountdownThemeConfig = {
      name: 'My Custom Theme',
      backgroundColor: '#0a0a2e',
      textColor: '#00ff88',
      fontFamily: 'monospace',
      countdownTexts: ['Ready...', 'Set...', 'Go!'],
      finalText: 'FLOW TIME!',
      animationPreset: 'fade',
      flowTitles: [
        "You're on fire!",
        "Keep that momentum going!",
        "Built different.",
        "The keyboard is smoking.",
        "Locked in.",
      ],
    }
    const blob = new Blob([JSON.stringify(reference, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'k2so-theme-reference.json'
    a.click()
    URL.revokeObjectURL(url)
  }, [])

  const handleDeleteTheme = useCallback(async (name: string) => {
    const updated = customThemes.filter((t) => t.name !== name)
    await updateTimerSetting('customThemes', updated)
    // If the deleted theme was active, fall back to rocket
    if (countdownTheme === name) {
      await updateTimerSetting('countdownTheme', 'rocket')
    }
  }, [customThemes, countdownTheme, updateTimerSetting])

  // Group entries by month
  const groupedEntries = useMemo(() => {
    const groups: Record<string, TimeEntry[]> = {}
    for (const entry of entries) {
      const date = new Date(entry.startTime * 1000)
      const key = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`
      if (!groups[key]) groups[key] = []
      groups[key].push(entry)
    }
    return Object.entries(groups).sort(([a], [b]) => b.localeCompare(a))
  }, [entries])

  const detectedTz = Intl.DateTimeFormat().resolvedOptions().timeZone

  return (
    <div>
      <div className="max-w-xl">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Timer</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Track work sessions. Click the clock icon in the top bar to start.
      </p>

      {/* Show timer button */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Show timer button</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Display the timer icon in the top bar</div>
        </div>
        <button
          onClick={() => updateTimerSetting('visible', !visible)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            visible ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              visible ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Enable countdown */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Countdown before start</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Show a themed 3-2-1 countdown before the timer begins</div>
        </div>
        <button
          onClick={() => updateTimerSetting('countdownEnabled', !countdownEnabled)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            countdownEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              countdownEnabled ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Countdown theme */}
      {countdownEnabled && (
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <div>
            <div className="text-xs text-[var(--color-text-primary)]">Countdown theme</div>
          </div>
          <SettingDropdown
            value={countdownTheme}
            options={[
              ...BUILT_IN_THEMES.map((t) => ({ value: t.value, label: t.label })),
              ...customThemes.map((t) => ({ value: t.name, label: t.name })),
            ]}
            onChange={(v) => updateTimerSetting('countdownTheme', v)}
          />
        </div>
      )}

      {/* Custom themes */}
      {countdownEnabled && (
        <div className="py-2 border-b border-[var(--color-border)]">
          <input
            ref={themeInputRef}
            type="file"
            accept=".json"
            className="hidden"
            onChange={handleThemeFileChange}
          />
          <div className="flex items-center justify-between mb-2">
            <div className="text-xs text-[var(--color-text-primary)]">Custom themes</div>
            <div className="flex items-center gap-3">
              <button
                onClick={handleDownloadReference}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
              >
                Download reference
              </button>
              <button
                onClick={handleUploadTheme}
                className="text-[10px] text-[var(--color-accent)] hover:underline cursor-pointer"
              >
                Upload .json
              </button>
            </div>
          </div>
          {customThemes.length === 0 ? (
            <div className="text-[10px] text-[var(--color-text-muted)]">No custom themes uploaded</div>
          ) : (
            <div className="space-y-1">
              {customThemes.map((t) => (
                <div key={t.name} className="flex items-center justify-between text-xs">
                  <div className="flex items-center gap-2">
                    <div
                      className="w-3 h-3 border border-[var(--color-border)]"
                      style={{ backgroundColor: t.backgroundColor }}
                    />
                    <span className="text-[var(--color-text-secondary)]">{t.name}</span>
                  </div>
                  <button
                    onClick={() => handleDeleteTheme(t.name)}
                    className="text-[10px] text-red-400 hover:text-red-300 cursor-pointer"
                  >
                    Remove
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Skip memo */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Skip memo on stop</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Save time entries without asking for a note</div>
        </div>
        <button
          onClick={() => updateTimerSetting('skipMemo', !skipMemo)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            skipMemo ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              skipMemo ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Timezone */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Timezone</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">
            Times displayed in this timezone (detected: {detectedTz})
          </div>
        </div>
        <SettingDropdown
          value={timezone}
          options={COMMON_TIMEZONES.map((tz) => ({
            value: tz,
            label: tz === '' ? `Auto (${detectedTz})` : tz,
          }))}
          onChange={(v) => updateTimerSetting('timezone', v)}
        />
      </div>

      </div>{/* end max-w-xl */}

      {/* Timer History */}
      <div className="mt-6">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold text-[var(--color-text-primary)]">History</h3>
          <div className="flex gap-2">
            <button
              onClick={() => handleExport('csv')}
              className="text-[10px] px-2 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            >
              Export CSV
            </button>
            <button
              onClick={() => handleExport('json')}
              className="text-[10px] px-2 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            >
              Export JSON
            </button>
          </div>
        </div>

        {/* Filters */}
        <div className="flex gap-2 mb-3 items-end">
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">From</label>
            <input
              type="date"
              value={filterStart}
              onChange={(e) => setFilterStart(e.target.value)}
              className="text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-2 py-1 outline-none"
            />
          </div>
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">To</label>
            <input
              type="date"
              value={filterEnd}
              onChange={(e) => setFilterEnd(e.target.value)}
              className="text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-2 py-1 outline-none"
            />
          </div>
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">Project</label>
            <SettingDropdown
              value={filterProject}
              options={[
                { value: '', label: 'All projects' },
                ...projects.map((p) => ({ value: p.id, label: p.name })),
              ]}
              onChange={setFilterProject}
            />
          </div>
          {(filterStart || filterEnd || filterProject) && (
            <button
              onClick={() => { setFilterStart(''); setFilterEnd(''); setFilterProject('') }}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer pb-1"
            >
              Clear
            </button>
          )}
        </div>

        {/* Entries */}
        {entries.length === 0 ? (
          <div className="text-xs text-[var(--color-text-muted)] py-4 text-center">
            No time entries yet. Click the timer button to start tracking.
          </div>
        ) : (
          <div className="max-h-[600px] overflow-y-auto">
            {/* Column headers */}
            <div className="grid gap-x-3 px-2 py-1.5 border-b border-[var(--color-border)] sticky top-0 bg-[var(--color-bg)] z-10 text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider"
              style={{ gridTemplateColumns: '190px 190px 80px 100px 1fr 20px' }}
            >
              <span>Start</span>
              <span>End</span>
              <span>Duration</span>
              <span>Project</span>
              <span>Memo</span>
              <span />
            </div>

            <div className="space-y-3">
              {groupedEntries.map(([monthKey, monthEntries]) => {
                const [year, month] = monthKey.split('-')
                const monthLabel = new Date(Number(year), Number(month) - 1).toLocaleString('en-US', { month: 'long', year: 'numeric' })
                return (
                  <div key={monthKey}>
                    <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1 sticky top-[28px] bg-[var(--color-bg)] py-1 px-2 z-[5]">
                      {monthLabel}
                    </div>
                    <div>
                      {monthEntries.map((entry) => {
                        const project = projects.find((p) => p.id === entry.projectId)
                        const timeOpts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }
                        return (
                          <div
                            key={entry.id}
                            className="grid items-center gap-x-3 py-1.5 px-2 hover:bg-[var(--color-bg-elevated)] group text-xs font-mono"
                            style={{ gridTemplateColumns: '190px 190px 80px 100px 1fr 20px' }}
                          >
                            <span className="text-[var(--color-text-muted)] truncate">
                              {formatTimestamp(entry.startTime, timezone, timeOpts)}
                            </span>
                            <span className="text-[var(--color-text-muted)] truncate">
                              {formatTimestamp(entry.endTime, timezone, timeOpts)}
                            </span>
                            <span className="text-[var(--color-accent)]">
                              {formatDuration(entry.durationSeconds)}
                            </span>
                            <span className="text-[var(--color-text-muted)] truncate font-sans">
                              {project?.name || '—'}
                            </span>
                            <span className="text-[var(--color-text-secondary)] truncate font-sans">
                              {entry.memo || '—'}
                            </span>
                            <button
                              onClick={() => deleteEntry(entry.id)}
                              className="text-red-400/0 group-hover:text-red-400/60 hover:!text-red-400 transition-colors cursor-pointer text-center"
                              title="Delete entry"
                            >
                              ×
                            </button>
                          </div>
                        )
                      })}
                    </div>
                  </div>
                )
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

// ── Projects / Workspaces Section ───────────────────────────────────
function ProjectsSection(): React.JSX.Element {
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
  const [claudeMdHasK2so, setClaudeMdHasK2so] = useState(true) // assume true to avoid flash
  const [claudeMdAppending, setClaudeMdAppending] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  // Close editor when project changes (user navigated away without using back button)
  useEffect(() => {
    setAgentEditorOpen(false)
    setAgentEditorName('')
  }, [project.id])

  // Check if CLAUDE.md has k2so context
  useEffect(() => {
    let cancelled = false
    const claudeMdPath = `${project.path}/CLAUDE.md`
    invoke<{ content: string }>('fs_read_file', { path: claudeMdPath })
      .then((result) => {
        if (!cancelled) {
          setClaudeMdHasK2so(result.content.includes('.k2so') || result.content.includes('k2so'))
        }
      })
      .catch(() => {
        if (!cancelled) setClaudeMdHasK2so(false) // file doesn't exist
      })
    return () => { cancelled = true }
  }, [project.path, project.id])


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

  return (
    <>
    {cropImage && (
      <IconCropDialog
        imageDataUrl={cropImage}
        onConfirm={handleCropConfirm}
        onCancel={() => setCropImage(null)}
      />
    )}
    <div className="max-w-xl space-y-6">
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

        {/* CLAUDE.md */}
        <div className="pt-3 border-t border-[var(--color-border)]">
          <div className="flex items-center justify-between">
            <div>
              <span className="text-xs text-[var(--color-text-secondary)]">CLAUDE.md</span>
              <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
                Instructions that every Claude session reads on launch.
              </p>
            </div>
            <button
              onClick={() => { setAgentEditorName('__claude_md__'); setAgentEditorOpen(true) }}
              className="px-2.5 py-1 text-[11px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
            >
              Manage CLAUDE.md
            </button>
          </div>
          {!claudeMdHasK2so && (
            <button
              onClick={async () => {
                setClaudeMdAppending(true)
                try {
                  const claudeMdPath = `${project.path}/CLAUDE.md`
                  let existing = ''
                  try {
                    const result = await invoke<{ content: string }>('fs_read_file', { path: claudeMdPath })
                    existing = result.content
                  } catch { /* file doesn't exist */ }

                  const k2soSection = `\n\n<!-- K2SO Context -->\n## K2SO Workspace\n\nThis project is managed by [K2SO](https://k2so.sh), an AI workspace IDE.\n\n### Directory Structure\n- \`.k2so/\` — K2SO workspace configuration\n  - \`.k2so/agents/\` — Agent profiles and work queues\n  - \`.k2so/work/inbox/\` — Workspace-level work inbox\n  - \`.k2so/prds/\` — Product requirement documents\n  - \`.k2so/PROJECT.md\` — Shared project context for agents\n\n### K2SO CLI\nThe \`k2so\` command is available in your terminal for workspace operations:\n\`\`\`\nk2so work inbox              # View workspace inbox\nk2so work create --title "..." --body "..."  # Create work items\nk2so agents list             # List agents with work counts\nk2so agents running          # List all active CLI LLM sessions\nk2so delegate <agent> <file> # Assign work: creates worktree + launches agent\nk2so reviews                 # List pending reviews\nk2so review approve <agent> <branch>  # Merge + cleanup\nk2so terminal write <id> "msg"  # Send message to a running terminal\nk2so terminal read <id> --lines 50  # Read terminal buffer output\nk2so commit                  # AI-assisted commit review\nk2so settings                # Show workspace settings\n\`\`\`\n\nRun \`k2so --help\` for all available commands.\n<!-- End K2SO Context -->\n`

                  const newContent = existing + k2soSection
                  await invoke('fs_write_file', { path: claudeMdPath, content: newContent })
                  setClaudeMdHasK2so(true)
                } catch (err) {
                  console.error('[settings] Failed to append k2so context:', err)
                } finally {
                  setClaudeMdAppending(false)
                }
              }}
              disabled={claudeMdAppending}
              className="mt-2 w-full px-2.5 py-1.5 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer flex items-center justify-center gap-1.5 disabled:opacity-40"
            >
              <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                <path d="M12 5v14M5 12h14" />
              </svg>
              {claudeMdAppending ? 'Adding...' : 'Add K2SO Context to CLAUDE.md'}
            </button>
          )}
        </div>
      </SettingsGroup>

      {/* ── Group 2: Agent Settings — Mode tabs, Heartbeat, Agents list ── */}
      {useSettingsStore.getState().agenticSystemsEnabled && <SettingsGroup title="Agent Settings (BETA)">
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
                      await invoke('k2so_agents_generate_workspace_claude_md', {
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

          {/* State selector — only when a mode is active */}
          {(project.agentMode || 'off') !== 'off' && (
            <StateSelector projectId={project.id} currentStateId={project.stateId} />
          )}

          {/* Heartbeat — only when a mode is active */}
          {(project.agentMode || 'off') !== 'off' && (
            <div className="flex items-center justify-between py-2 border-t border-[var(--color-border)]">
              <div className="flex items-center gap-2">
                {/* Heartbeat indicator with pulse waves */}
                <div className="relative flex items-center justify-center w-5 h-5 flex-shrink-0 overflow-hidden">
                  <span className={`absolute w-5 h-5 rounded-full transition-opacity ${project.heartbeatEnabled ? 'bg-red-500/30 animate-[heartwave_1.2s_ease-out_infinite] opacity-100' : 'opacity-0'}`} />
                  <span className={`absolute w-5 h-5 rounded-full transition-opacity ${project.heartbeatEnabled ? 'bg-red-500/20 animate-[heartwave_1.2s_ease-out_0.3s_infinite] opacity-100' : 'opacity-0'}`} />
                  <span className={`relative w-2 h-2 rounded-full transition-colors ${
                    project.heartbeatEnabled ? 'bg-red-500 animate-[heartpulse_1.2s_ease-in-out_infinite]' : 'bg-red-500/25'
                  }`} />
                </div>
                <div>
                  <span className={`text-xs ${project.heartbeatMode !== 'off' ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>Heartbeat</span>
                  <p className={`text-[9px] ${project.heartbeatMode !== 'off' ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}`}>
                    {project.heartbeatMode !== 'off' ? 'Wakes up automatically to work' : 'Only works when manually launched'}
                  </p>
                </div>
              </div>
              <button
                onClick={() => {
                  import('@/stores/heartbeat-schedule').then(({ useHeartbeatScheduleStore }) => {
                    useHeartbeatScheduleStore.getState().open(project.id)
                  })
                }}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)] px-2 py-0.5 transition-colors no-drag cursor-pointer"
              >
                {project.heartbeatMode !== 'off'
                  ? (() => {
                      try {
                        const s = project.heartbeatSchedule ? JSON.parse(project.heartbeatSchedule) : null
                        const to12h = (t: string) => { const [hh, mm] = t.split(':'); let h = parseInt(hh); const ap = h >= 12 ? 'PM' : 'AM'; if (h === 0) h = 12; else if (h > 12) h -= 12; return mm === '00' ? `${h} ${ap}` : `${h}:${mm} ${ap}` }
                        if (project.heartbeatMode === 'hourly' && s) {
                          const secs = s.every_seconds ?? 300
                          const freq = secs >= 3600 ? `${Math.round(secs / 3600)}h` : `${Math.round(secs / 60)}m`
                          return `Every ${freq}`
                        }
                        if (s?.frequency) return `${s.frequency.charAt(0).toUpperCase() + s.frequency.slice(1)} ${to12h(s.time ?? '09:00')}`
                      } catch {}
                      return project.heartbeatMode
                    })()
                  : 'Configure'}
              </button>
            </div>
          )}

          {/* Adaptive Heartbeat Config — removed, now handled by HeartbeatScheduleDialog */}

          {/* Custom Agent persona — only in Custom Agent mode */}
          {(project.agentMode || 'off') === 'custom' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <CustomAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* K2SO Agent persona — only in K2SO Agent mode */}
          {(project.agentMode || 'off') === 'agent' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <K2SOAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

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
    </>
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
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {content || '*No content yet. Use the AI assistant to set up your project context.*'}
                </ReactMarkdown>
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

// ── CLAUDE.md Editor (AIFileEditor for workspace root CLAUDE.md) ──────

function ClaudeMdEditor({ projectPath, projectName, onClose }: { projectPath: string; projectName: string; onClose: () => void }): React.JSX.Element {
  const [content, setContent] = useState('')
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  const filePath = `${projectPath}/CLAUDE.md`
  const watchDir = projectPath

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

  const systemPrompt = useMemo(() => [
    `You're helping the user write their CLAUDE.md file for the "${projectName}" workspace.`,
    ``,
    `File: CLAUDE.md (project root)`,
    ``,
    `This file is automatically read by Claude Code at the start of every session.`,
    `It should contain project-specific instructions, conventions, and context:`,
    ``,
    `• Project overview — what this codebase does`,
    `• Tech stack — languages, frameworks, key dependencies`,
    `• Key directories — important paths and what lives in them`,
    `• Conventions — code style, commit format, branch naming, PR process`,
    `• Build & test — how to build, run tests, deploy`,
    `• Important notes — gotchas, known issues, things to watch out for`,
    ``,
    `Edit CLAUDE.md in the current directory. The user sees a live preview on the right.`,
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
        `Open and read CLAUDE.md in the current directory. Help the user define their project context for "${projectName}". Start by asking about their tech stack and project structure.`,
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
      title={`CLAUDE.md: ${projectName}`}
      instructions="Editing CLAUDE.md — read by Claude Code at the start of every session. Note: K2SO regenerates parts of this file on launch. For persistent custom instructions, use .k2so/PROJECT.md instead."
      warningText="This file is partially auto-generated by K2SO. Custom edits may be overwritten on next launch."
      onFileChange={handleFileChange}
      onClose={onClose}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">CLAUDE.md</span>
              <span className="mx-2">&middot;</span>
              <span>Session context</span>
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
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {content || '*No CLAUDE.md yet. Use the AI assistant to set up your project context, or click Edit to write it manually.*'}
                </ReactMarkdown>
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
  const [ready, setReady] = useState(false)
  const [agentName, setAgentName] = useState(projectName.toLowerCase().replace(/\s+/g, '-'))

  // Ensure the single custom agent exists for this workspace
  useEffect(() => {
    const ensure = async () => {
      try {
        const agents = await invoke<(K2soAgentInfo & { agentType?: string })[]>('k2so_agents_list', { projectPath })
        const existing = agents.find((a: any) => a.agentType === 'custom')
        if (existing) {
          setAgentName(existing.name)
        } else {
          const name = projectName.toLowerCase().replace(/\s+/g, '-')
          await invoke('k2so_agents_create', {
            projectPath,
            name,
            role: 'Custom agent — customize via the persona editor',
            agentType: 'custom',
          })
          setAgentName(name)
        }
        setReady(true)
      } catch (e) {
        console.error('[custom-agent] Init failed:', e)
        setReady(true)
      }
    }
    ensure()
  }, [projectPath, projectName])

  return (
    <div className="flex items-center justify-between gap-3">
      <p className="text-[10px] text-[var(--color-text-muted)]">
        Define what this agent does when it wakes up on the heartbeat.
      </p>
      <button
        onClick={() => onOpenEditor(agentName)}
        disabled={!ready}
        className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0"
      >
        ✎ Manage Persona
      </button>
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
        ✎ Manage Persona
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
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Connected Workspaces
        </h3>
        {availableProjects.length > 0 && (
          <button
            onClick={() => { setShowAdd(!showAdd); setSearch('') }}
            className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10 transition-colors no-drag cursor-pointer"
            title="Add connection"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
              <line x1="5" y1="1" x2="5" y2="9" />
              <line x1="1" y1="5" x2="9" y2="5" />
            </svg>
          </button>
        )}
      </div>

      <p className="text-[10px] text-[var(--color-text-muted)]">
        Connect other workspaces so this agent can oversee or interact with them.
      </p>

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

// ── Shared components ────────────────────────────────────────────────
// ── Reusable Dropdown (matches FocusGroupDropdown style) ─────────────

function SettingDropdown({
  value,
  options,
  onChange,
  className,
}: {
  value: string
  options: { value: string; label: string }[]
  onChange: (value: string) => void | Promise<void>
  className?: string
}): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  const selected = options.find((o) => o.value === value) ?? options[0]

  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent): void => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  return (
    <div ref={containerRef} className={`relative no-drag ${className ?? ''}`}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-2 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-primary)] transition-colors cursor-pointer"
      >
        <span className="truncate">{selected?.label ?? ''}</span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {isOpen && (
        <div className="absolute top-full right-0 z-50 mt-0.5 min-w-full bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-xl max-h-60 overflow-y-auto">
          {options.map((option) => {
            const isActive = option.value === value
            return (
              <button
                key={option.value}
                onClick={() => { onChange(option.value); setIsOpen(false) }}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors cursor-pointer ${
                  isActive
                    ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
                }`}
              >
                <span className="truncate flex-1">{option.label}</span>
                {isActive && (
                  <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

// ── Claude Auth Refresh Toggle ──────────────────────────────────────

function AgenticSystemsToggle(): React.JSX.Element {
  const enabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  const toggle = async () => {
    const newVal = !enabled
    useSettingsStore.setState({ agenticSystemsEnabled: newVal })
    await invoke('settings_update', { updates: { agenticSystemsEnabled: newVal } }).catch(console.error)
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Agentic Systems <span className="text-[9px] text-[var(--color-text-muted)]">(BETA)</span></span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {enabled
            ? 'AI agents, workspace manager, heartbeat, and review queue are active'
            : 'Enable to unlock AI agent orchestration across workspaces'}
        </p>
      </div>
      <button
        onClick={toggle}
        className="no-drag cursor-pointer flex-shrink-0 relative"
        style={{
          width: 36,
          height: 20,
          backgroundColor: enabled ? 'var(--color-accent)' : '#333',
          border: 'none',
          transition: 'background-color 150ms',
        }}
      >
        <span
          style={{
            position: 'absolute',
            top: 2,
            left: enabled ? 18 : 2,
            width: 16,
            height: 16,
            backgroundColor: '#fff',
            transition: 'left 150ms',
          }}
        />
      </button>
    </div>
  )
}

function ClaudeAuthRefreshRow(): React.JSX.Element {
  const claudeAuthAutoRefresh = useSettingsStore((s) => s.claudeAuthAutoRefresh)
  const setClaudeAuthAutoRefresh = useSettingsStore((s) => s.setClaudeAuthAutoRefresh)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const {
    state: authState,
    secondsRemaining,
    refreshing,
    fetchStatus,
    refresh,
    installScheduler,
    uninstallScheduler,
  } = useClaudeAuthStore()

  // Poll status every 60s while mounted
  useEffect(() => {
    fetchStatus()
    const interval = setInterval(fetchStatus, 60_000)
    return () => clearInterval(interval)
  }, [fetchStatus])

  const handleToggle = useCallback(async () => {
    if (!claudeAuthAutoRefresh) {
      // Enabling — show consent dialog
      const confirmed = await confirm({
        title: 'Install Background Token Refresh?',
        message:
          'K2SO will install a background scheduler that refreshes your Claude authentication token every 20 minutes, preventing session expiry.\n\nThis runs independently of K2SO and can be disabled at any time from Settings.',
        confirmLabel: 'Install',
      })
      if (!confirmed) return
      try {
        await installScheduler()
        setClaudeAuthAutoRefresh(true)
        fetchStatus()
      } catch (e) {
        console.error('[settings] Failed to install Claude auth scheduler:', e)
      }
    } else {
      // Disabling
      try {
        await uninstallScheduler()
        setClaudeAuthAutoRefresh(false)
        fetchStatus()
      } catch (e) {
        console.error('[settings] Failed to uninstall Claude auth scheduler:', e)
      }
    }
  }, [claudeAuthAutoRefresh, confirm, installScheduler, uninstallScheduler, setClaudeAuthAutoRefresh, fetchStatus])

  const handleRefreshNow = useCallback(async () => {
    await refresh()
    fetchStatus()
  }, [refresh, fetchStatus])

  // Status display
  const statusDot = (color: string) => (
    <span className="w-1.5 h-1.5 flex-shrink-0" style={{ backgroundColor: color }} />
  )

  let statusIndicator: React.ReactNode = null
  if (authState !== 'unknown') {
    const remaining = secondsRemaining ?? 0
    const minutes = Math.floor(Math.abs(remaining) / 60)

    const config: Record<ClaudeAuthState, { color: string; text: string }> = {
      valid: { color: '#22c55e', text: `Valid (${minutes}m)` },
      expiring: { color: '#eab308', text: 'Expiring soon' },
      expired: { color: '#ef4444', text: 'Expired' },
      missing: { color: '#6b7280', text: 'No credentials' },
      unknown: { color: '#6b7280', text: '' },
    }

    const { color, text } = config[authState]
    statusIndicator = (
      <div className="flex items-center gap-1.5 mr-3">
        {statusDot(color)}
        <span className="text-[10px] text-[var(--color-text-muted)] whitespace-nowrap">{text}</span>
        {(authState === 'expiring' || authState === 'expired') && (
          <button
            onClick={handleRefreshNow}
            disabled={refreshing}
            className="text-[10px] text-[var(--color-accent)] hover:underline cursor-pointer no-drag disabled:opacity-50"
          >
            {refreshing ? '...' : 'Refresh'}
          </button>
        )}
      </div>
    )
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Auto-refresh Claude credentials</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Background scheduler keeps your Claude session alive
        </p>
      </div>
      <div className="flex items-center flex-shrink-0">
        {statusIndicator}
        <button
          onClick={handleToggle}
          className="no-drag cursor-pointer flex-shrink-0 relative"
          style={{
            width: 36,
            height: 20,
            backgroundColor: claudeAuthAutoRefresh ? 'var(--color-accent)' : '#333',
            border: 'none',
            transition: 'background-color 150ms',
          }}
        >
          <span
            style={{
              position: 'absolute',
              top: 2,
              left: claudeAuthAutoRefresh ? 18 : 2,
              width: 16,
              height: 16,
              backgroundColor: '#fff',
              transition: 'left 150ms',
            }}
          />
        </button>
      </div>
    </div>
  )
}

function SettingRow({
  label,
  children
}: {
  label: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">{label}</span>
      {children}
    </div>
  )
}

function SettingsGroup({
  title,
  children
}: {
  title: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
        {title}
      </h3>
      <div className="ml-2 pl-3 border-l-2 border-[var(--color-border)] space-y-1">
        {children}
      </div>
    </div>
  )
}

// ── Local LLM Settings (shared between General and AI Assistant) ────

function LocalLLMSettings(): React.JSX.Element {
  const { isDownloading, downloadProgress, modelLoaded } = useAssistantStore()
  const aiAssistantEnabled = useSettingsStore((s) => s.aiAssistantEnabled)
  const setAiAssistantEnabled = useSettingsStore((s) => s.setAiAssistantEnabled)
  const [modelPath, setModelPath] = useState<string | null>(null)
  const [modelExists, setModelExists] = useState<boolean | null>(null)
  const [customPath, setCustomPath] = useState('')
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingModel, setLoadingModel] = useState(false)

  useEffect(() => {
    invoke<{ loaded: boolean; modelPath: string | null; downloading: boolean }>('assistant_status')
      .then((status) => {
        setModelPath(status.modelPath)
        if (status.modelPath) setCustomPath(status.modelPath)
      })
      .catch((e) => console.warn('[settings]', e))

    invoke<boolean>('assistant_check_model')
      .then((exists) => setModelExists(exists))
      .catch((e) => console.warn('[settings]', e))
  }, [modelLoaded])

  const handleDownload = useCallback(async () => {
    try {
      setLoadError(null)
      await invoke('assistant_download_default_model')
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  const handleLoadCustom = useCallback(async () => {
    if (!customPath.trim()) return
    setLoadingModel(true)
    setLoadError(null)
    try {
      const finalPath = await invoke<string>('assistant_load_model', { path: customPath.trim() })
      setModelPath(finalPath)
      setCustomPath(finalPath)
      useAssistantStore.getState().setModelLoaded(true)
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoadingModel(false)
    }
  }, [customPath])

  return (
    <div>
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">AI Workspace Assistant</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        A local LLM that translates natural language into workspace operations. Press <kbd className="px-1 py-0.5 bg-white/[0.06] text-[var(--color-text-secondary)] font-mono text-[10px]">&#8984;L</kbd> to open.
        Runs entirely on your machine — no data is sent to external servers.
      </p>
      <div className="border border-[var(--color-border)]">
        {/* Enabled */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-primary)]">Enabled</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Disabling saves battery by not loading the model into memory</p>
          </div>
          <button
            onClick={() => setAiAssistantEnabled(!aiAssistantEnabled)}
            className="no-drag cursor-pointer flex-shrink-0 relative"
            style={{
              width: 36,
              height: 20,
              backgroundColor: aiAssistantEnabled ? 'var(--color-accent)' : '#333',
              border: 'none',
              transition: 'background-color 150ms'
            }}
          >
            <span
              style={{
                position: 'absolute',
                top: 2,
                left: aiAssistantEnabled ? 18 : 2,
                width: 16,
                height: 16,
                backgroundColor: '#fff',
                transition: 'left 150ms'
              }}
            />
          </button>
        </div>
        {/* Model Status */}
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-primary)]">Model Status</span>
          <div className="flex items-center gap-2 mt-2">
            <span
              className="w-2 h-2 flex-shrink-0"
              style={{ backgroundColor: modelLoaded ? '#4ade80' : '#ef4444' }}
            />
            <span className="text-xs text-[var(--color-text-secondary)]">
              {modelLoaded ? 'Model loaded and ready' : 'No model loaded'}
            </span>
          </div>
          {modelPath && (
            <p className="text-[10px] font-mono text-[var(--color-text-muted)] break-all mt-1">
              {modelPath}
            </p>
          )}
        </div>
        {/* Default Model */}
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-primary)]">Default Model</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-1 mb-2">
              Qwen2.5-1.5B-Instruct (Q4_K_M) — ~1.1GB download. Runs locally with Metal GPU acceleration.
            </p>
            {isDownloading ? (
              <div>
                <div className="flex items-center justify-between mb-1">
                  <span className="text-xs text-[var(--color-text-secondary)]">Downloading...</span>
                  <span className="text-xs font-mono text-[var(--color-text-muted)]">{Math.round(downloadProgress)}%</span>
                </div>
                <div className="h-1.5 bg-[var(--color-bg)] overflow-hidden">
                  <div
                    className="h-full bg-[var(--color-accent)] transition-all duration-300"
                    style={{ width: `${downloadProgress}%` }}
                  />
                </div>
              </div>
            ) : (
              <button
                onClick={handleDownload}
                disabled={modelExists === true && modelLoaded}
                className="px-3 py-1.5 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] hover:bg-white/[0.08] transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-default no-drag"
              >
                {modelExists ? (modelLoaded ? 'Downloaded & Loaded' : 'Download & Load') : 'Download Default Model'}
              </button>
            )}
          </div>
          {/* Custom Model */}
          <div className="px-4 py-3">
            <span className="text-xs text-[var(--color-text-primary)]">Custom Model</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-1 mb-2">
              Point to any GGUF model file. It will be copied to <span className="font-mono">~/.k2so/models/</span> automatically.
            </p>
            <div className="flex gap-2">
              <input
                type="text"
                value={customPath}
                onChange={(e) => setCustomPath(e.target.value)}
                placeholder="~/.k2so/models/your-model.gguf"
                className="flex-1 px-2 py-1.5 text-xs font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
              />
              <button
                onClick={handleLoadCustom}
                disabled={!customPath.trim() || loadingModel}
                className="px-3 py-1.5 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] hover:bg-white/[0.08] transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-default no-drag flex-shrink-0"
              >
                {loadingModel ? 'Loading...' : 'Load'}
              </button>
            </div>
          </div>
        </div>
      {/* Error Display */}
      {loadError && (
        <div className="p-2 text-xs text-red-400 bg-red-500/5 border border-red-500/20 mt-3">
          {loadError}
        </div>
      )}
    </div>
  )
}


// ── Mobile Companion Section ────────────────────────────────────────

function CompanionSection(): React.JSX.Element {
  const [enabled, setEnabled] = useState(false)
  const [autoStart, setAutoStart] = useState(false)
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [passwordSet, setPasswordSet] = useState(false)
  const [ngrokToken, setNgrokToken] = useState('')
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null)
  const [connectedClients, setConnectedClients] = useState(0)
  const [sessions, setSessions] = useState<Array<{ token: string; remoteAddr: string; createdAt: string }>>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [urlCopied, setUrlCopied] = useState(false)

  useEffect(() => {
    const load = async () => {
      try {
        const settings = await invoke<any>('settings_get')
        const c = settings?.companion || {}
        setUsername(c.username || '')
        setPasswordSet(!!(c.passwordHash))
        setNgrokToken(c.ngrokAuthToken || '')
        setAutoStart(c.autoStart || false)
      } catch { /* ignore */ }
      try {
        const status = await invoke<any>('companion_status')
        if (status.running) {
          setEnabled(true)
          if (status.tunnelUrl) {
            setTunnelUrl(status.tunnelUrl)
            setConnectedClients(status.connectedClients || 0)
            setSessions(status.sessions || [])
          }
        } else {
          setEnabled(false)
        }
      } catch { /* ignore */ }
    }
    load()
  }, [])

  useEffect(() => {
    // Poll companion status — runs always when autoStart is on (to detect auto-started companion)
    // or when enabled (to track running companion)
    if (!enabled && !autoStart) return
    const interval = setInterval(async () => {
      try {
        const status = await invoke<any>('companion_status')
        if (!status.running) {
          if (enabled) {
            // Tunnel genuinely stopped
            setEnabled(false)
            setTunnelUrl(null)
            setConnectedClients(0)
            setSessions([])
          }
        } else {
          // Companion is running — make sure UI reflects it
          if (!enabled) setEnabled(true)
          if (status.tunnelUrl) {
            setTunnelUrl(status.tunnelUrl)
            setConnectedClients(status.connectedClients || 0)
            setSessions(status.sessions || [])
          }
        }
      } catch { /* ignore */ }
    }, 5000)
    return () => clearInterval(interval)
  }, [enabled, autoStart])

  const handleToggle = async () => {
    setLoading(true)
    setError(null)
    try {
      if (enabled) {
        await invoke('companion_stop')
        setEnabled(false)
        setTunnelUrl(null)
        setConnectedClients(0)
        setSessions([])
        await invoke('settings_update', { updates: { companion: { enabled: false } } })
      } else {
        await invoke('settings_update', {
          updates: { companion: { enabled: true, username, ngrokAuthToken: ngrokToken } }
        })
        const url = await invoke<string>('companion_start')
        setEnabled(true)
        setTunnelUrl(url)
      }
    } catch (err: any) {
      setError(typeof err === 'string' ? err : err?.message || 'Failed')
    } finally {
      setLoading(false)
    }
  }

  const handleSetPassword = async () => {
    if (!password) return
    try {
      await invoke('companion_set_password', { password })
      setPasswordSet(true)
      setPassword('')
    } catch (err: any) {
      setError(typeof err === 'string' ? err : 'Failed to set password')
    }
  }

  const handleDisconnect = async (token: string) => {
    try {
      await invoke('companion_disconnect_session', { sessionToken: token })
      setSessions((prev) => prev.filter((s) => s.token !== token))
    } catch { /* ignore */ }
  }

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Mobile Companion</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        Access your K2SO agents remotely through the companion app. Requires an ngrok account.
      </p>

      <div className="flex items-center gap-2 mb-4 px-3 py-2 border border-[var(--color-border)]">
        <span className="w-2 h-2 flex-shrink-0 rounded-full" style={{ backgroundColor: tunnelUrl ? '#22c55e' : enabled ? '#eab308' : '#6b7280' }} />
        <span className="text-xs text-[var(--color-text-secondary)]">
          {tunnelUrl ? `Connected (${connectedClients} client${connectedClients !== 1 ? 's' : ''})` : enabled ? 'Connecting...' : 'Not running'}
        </span>
        {tunnelUrl && (
          <div className="flex items-center gap-1.5 ml-auto">
            <span className="text-[10px] text-[var(--color-text-muted)] font-mono truncate max-w-[200px]">{tunnelUrl}</span>
            <button
              onClick={() => {
                navigator.clipboard.writeText(tunnelUrl).then(() => {
                  setUrlCopied(true)
                  setTimeout(() => setUrlCopied(false), 1500)
                }).catch(() => {})
              }}
              className={`text-[10px] no-drag cursor-pointer ${urlCopied ? 'text-green-400' : 'text-[var(--color-accent)] hover:underline'}`}
            >
              {urlCopied ? 'Copied!' : 'Copy'}
            </button>
          </div>
        )}
      </div>

      {error && <div className="text-[10px] text-red-400 mb-3 px-3 py-1.5 border border-red-400/20 bg-red-400/5">{error}</div>}

      <div className="space-y-0">
        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Enable Companion</span>
          <button onClick={handleToggle} disabled={loading} className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'} ${loading ? 'opacity-50' : ''}`}>
            <span className={`w-2.5 h-2.5 bg-white block transition-transform ${enabled ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Start on Launch</span>
            <p className="text-[10px] text-[var(--color-text-muted)]">Automatically connect when K2SO opens</p>
          </div>
          <button
            onClick={() => {
              const next = !autoStart
              setAutoStart(next)
              invoke('settings_update', { updates: { companion: { autoStart: next } } }).catch(() => {})
            }}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${autoStart ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'}`}
          >
            <span className={`w-2.5 h-2.5 bg-white block transition-transform ${autoStart ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Username</span>
          <input type="text" value={username} onChange={(e) => setUsername(e.target.value)} onBlur={() => invoke('settings_update', { updates: { companion: { username } } }).catch(() => {})} placeholder="Enter username" className="w-48 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Password</span>
            {passwordSet && <span className="ml-2 text-[10px] text-green-400">Set</span>}
          </div>
          <div className="flex items-center gap-1.5">
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') handleSetPassword() }} placeholder={passwordSet ? '••••••••' : 'Enter password'} className="w-36 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
            {password && <button onClick={handleSetPassword} className="px-2 py-1 text-[10px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer">Save</button>}
          </div>
        </div>

        <div className="flex items-center justify-between py-2.5 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">ngrok Auth Token</span>
          <input type="password" value={ngrokToken} onChange={(e) => setNgrokToken(e.target.value)} onBlur={() => invoke('settings_update', { updates: { companion: { ngrokAuthToken: ngrokToken } } }).catch(() => {})} placeholder="Enter ngrok token" className="w-48 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag" />
        </div>
      </div>

      {sessions.length > 0 && (
        <div className="mt-6">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">Active Sessions</h3>
          <div className="border border-[var(--color-border)]">
            {sessions.map((session) => (
              <div key={session.token} className="flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
                <div className="flex items-center gap-2">
                  <span className="w-1.5 h-1.5 bg-green-400 rounded-full flex-shrink-0" />
                  <span className="text-xs text-[var(--color-text-primary)] font-mono">{session.remoteAddr}</span>
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    {(() => { const ago = Math.floor((Date.now() - new Date(session.createdAt).getTime()) / 60000); return ago < 1 ? 'just now' : ago < 60 ? `${ago}m ago` : `${Math.floor(ago / 60)}h ago` })()}
                  </span>
                </div>
                <button onClick={() => handleDisconnect(session.token)} className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer">Disconnect</button>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="mt-6 text-[10px] text-[var(--color-text-muted)] space-y-1">
        <p>1. Create a free account at <span className="text-[var(--color-accent)]">ngrok.com</span> and copy your auth token.</p>
        <p>2. Set a username and password for the companion app to authenticate.</p>
        <p>3. Enable the toggle — K2SO will create a secure tunnel and show you the URL.</p>
        <p>4. Enter the URL in the K2SO companion app on your phone.</p>
      </div>
    </div>
  )
}

// ── Agent Skills Section ─────────────────────────────────────────────

type SkillTier = 'manager' | 'agent_template' | 'custom_agent'

interface SkillLayerInfo {
  filename: string
  title: string
  preview: string
  path: string
}

const SKILL_TABS: { key: SkillTier; label: string }[] = [
  { key: 'manager', label: 'Workspace Manager' },
  { key: 'agent_template', label: 'Agent Template' },
  { key: 'custom_agent', label: 'Custom Agent' },
]

const LOCKED_LAYERS: Record<SkillTier, string[]> = {
  manager: [
    'Identity + Workspace State',
    'Connected Workspaces',
    'Team Roster',
    'Standing Orders',
    'Decision Framework',
    'Delegation + Review',
    'Communication Commands',
  ],
  agent_template: [
    'Identity',
    'Check In + Status + Done',
    'File Reservations',
  ],
  custom_agent: [
    'Identity',
    'Check In + Status + Done',
    'Cross-Workspace Messaging',
    'File Reservations',
  ],
}

// Static content descriptions for locked layers (shown in preview)
const LOCKED_LAYER_DESCRIPTIONS: Record<string, string> = {
  'Identity + Workspace State': '**Auto-generated per workspace.**\n\nIncludes the workspace name, current mode (Build/Managed/Maintenance/Locked), and mode description. Each workspace gets unique identity context.',
  'Connected Workspaces': '**Auto-generated per workspace.**\n\nLists workspaces connected via workspace relations — both outgoing (workspaces this manager oversees) and incoming (agents that communicate with this workspace).',
  'Team Roster': '**Auto-generated per workspace.**\n\nLists all agent templates in this workspace with their names and roles. The manager uses this to decide which specialist to delegate work to.',
  'Standing Orders': '**Auto-generated (same for all managers).**\n\n9-step checklist run on every wake cycle:\n1. `k2so checkin`\n2. Triage messages\n3. Triage work items by priority\n4. Handle simple tasks directly\n5. Delegate complex tasks\n6. Check active agents\n7. Review completed work\n8. Update status\n9. Mark done or blocked',
  'Decision Framework': '**Auto-generated (same for all managers).**\n\nTwo decision axes:\n- **By complexity**: Simple (work directly) vs Complex (delegate)\n- **By workspace mode**: Build (full autonomy), Managed (features need approval), Maintenance (bugs only), Locked (no activity)',
  'Delegation + Review': '**Auto-generated (same for all managers).**\n\nDelegation: choose agent → create work item → `k2so delegate` → agent works in worktree → review.\n\nReview: `k2so review approve/reject/feedback` for completed agent work.',
  'Communication Commands': '**Auto-generated (same for all managers).**\n\nCore commands: `k2so checkin`, `k2so status`, `k2so done`, `k2so msg`, `k2so reserve`, `k2so release`.',
  'Identity': '**Auto-generated per agent.**\n\nThe agent name and workspace it belongs to.',
  'Check In + Status + Done': '**Auto-generated (same for all).**\n\n`k2so checkin` — wake up briefing\n`k2so status "msg"` — report progress\n`k2so done` / `k2so done --blocked "reason"` — complete or block task',
  'File Reservations': '**Auto-generated (same for all).**\n\n`k2so reserve <paths>` — claim files for exclusive editing\n`k2so release` — release claims',
  'Cross-Workspace Messaging': '**Auto-generated (same for all custom agents).**\n\n`k2so msg <workspace>:inbox "text"` — send work to connected workspaces\n`k2so msg --wake` — urgent delivery with agent wake-up',
}

function AgentSkillsSection(): React.JSX.Element {
  const [activeTier, setActiveTier] = useState<SkillTier>('manager')
  const [layers, setLayers] = useState<SkillLayerInfo[]>([])
  const [adding, setAdding] = useState(false)
  const [newName, setNewName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const [selectedLayer, setSelectedLayer] = useState<{ type: 'locked' | 'user'; name: string; filename?: string } | null>(null)
  const [previewContent, setPreviewContent] = useState<string>('')

  const loadLayers = useCallback(async (tier: SkillTier) => {
    try {
      const list = await invoke<SkillLayerInfo[]>('skill_layers_list', { tier })
      setLayers(list)
    } catch (err) {
      console.error('[agent-skills] Failed to load layers:', err)
      setLayers([])
    }
  }, [])

  useEffect(() => {
    loadLayers(activeTier)
  }, [activeTier, loadLayers])

  useEffect(() => {
    if (adding && inputRef.current) {
      inputRef.current.focus()
    }
  }, [adding])

  useEffect(() => {
    if (toast) {
      const t = setTimeout(() => setToast(null), 2000)
      return () => clearTimeout(t)
    }
  }, [toast])

  const handleCreate = useCallback(async () => {
    const name = newName.trim()
    if (!name) return
    try {
      await invoke<SkillLayerInfo>('skill_layers_create', { tier: activeTier, name })
      setNewName('')
      setAdding(false)
      loadLayers(activeTier)
    } catch (err) {
      console.error('[agent-skills] Create failed:', err)
    }
  }, [newName, activeTier, loadLayers])

  const handleDelete = useCallback(async (filename: string) => {
    try {
      await invoke('skill_layers_delete', { tier: activeTier, filename })
      setConfirmDelete(null)
      loadLayers(activeTier)
    } catch (err) {
      console.error('[agent-skills] Delete failed:', err)
    }
  }, [activeTier, loadLayers])

  const handleEdit = useCallback((layer: SkillLayerInfo) => {
    navigator.clipboard.writeText(layer.path).then(() => {
      setToast('Copied path — open in your editor')
    }).catch(() => {
      setToast(layer.path)
    })
  }, [])

  // Load preview content when a layer is selected
  useEffect(() => {
    if (!selectedLayer) { setPreviewContent(''); return }
    if (selectedLayer.type === 'locked') {
      setPreviewContent(LOCKED_LAYER_DESCRIPTIONS[selectedLayer.name] || `*Auto-generated section: ${selectedLayer.name}*`)
    } else if (selectedLayer.filename) {
      invoke<string>('skill_layers_get_content', { tier: activeTier, filename: selectedLayer.filename })
        .then((content) => setPreviewContent(content || '*Empty layer — click Edit to add content.*'))
        .catch(() => setPreviewContent('*Failed to load content.*'))
    }
  }, [selectedLayer, activeTier])

  // Clear selection on tier change
  useEffect(() => { setSelectedLayer(null) }, [activeTier])

  const locked = LOCKED_LAYERS[activeTier]

  return (
    <div className="max-w-3xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Agent Skills</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        Skill layers are injected into agent system prompts. Click a layer to preview its content.
      </p>

      {/* Tier tabs */}
      <div className="flex gap-1 mb-4">
        {SKILL_TABS.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => { setActiveTier(key); setAdding(false); setConfirmDelete(null) }}
            className={`px-3 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
              activeTier === key
                ? 'bg-[var(--color-accent)] text-white'
                : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Split layout: layer list + preview */}
      <div className="flex gap-3">
        {/* Left: Hamburger layer list */}
        <div className="border border-[var(--color-border)] flex-1 min-w-0">
        {/* Locked layers */}
        {locked.map((name, i) => {
          const isSelected = selectedLayer?.type === 'locked' && selectedLayer.name === name
          return (
          <div
            key={`locked-${i}`}
            className={`flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer transition-colors ${
              isSelected ? 'bg-white/[0.06] text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)] opacity-50 hover:opacity-70'
            }`}
            onClick={() => setSelectedLayer({ type: 'locked', name })}
          >
            <div className="flex items-center gap-2">
              <span className="w-1 h-4 bg-[var(--color-text-muted)]/30 rounded-sm flex-shrink-0" />
              <span className="text-xs">{name}</span>
            </div>
            <span className="text-[10px] italic">auto</span>
          </div>
          )
        })}

        {/* User layers */}
        {layers.map((layer) => {
          const isSelected = selectedLayer?.type === 'user' && selectedLayer.filename === layer.filename
          return (
          <div
            key={layer.filename}
            className={`flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer transition-colors ${
              isSelected ? 'bg-white/[0.06]' : 'hover:bg-white/[0.03]'
            }`}
            onClick={() => setSelectedLayer({ type: 'user', name: layer.title, filename: layer.filename })}
          >
            <div className="flex items-center gap-2 min-w-0">
              <span className="w-1 h-4 bg-[var(--color-accent)] rounded-sm flex-shrink-0" />
              <div className="min-w-0">
                <span className="text-xs text-[var(--color-text-primary)] block truncate">{layer.title}</span>
                {layer.preview && (
                  <span className="text-[10px] text-[var(--color-text-muted)] block truncate">{layer.preview}</span>
                )}
              </div>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {confirmDelete === layer.filename ? (
                <>
                  <span className="text-[10px] text-[var(--color-text-muted)]">Delete?</span>
                  <button
                    onClick={() => handleDelete(layer.filename)}
                    className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer"
                  >
                    Yes
                  </button>
                  <button
                    onClick={() => setConfirmDelete(null)}
                    className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                  >
                    No
                  </button>
                </>
              ) : (
                <>
                  <button
                    onClick={() => handleEdit(layer)}
                    className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
                  >
                    Edit
                  </button>
                  <button
                    onClick={() => setConfirmDelete(layer.filename)}
                    className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer"
                  >
                    Delete
                  </button>
                </>
              )}
            </div>
          </div>
          )
        })}

        {/* Add layer inline input */}
        {adding ? (
          <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
            <input
              ref={inputRef}
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleCreate()
                if (e.key === 'Escape') { setAdding(false); setNewName('') }
              }}
              placeholder="Layer name..."
              className="flex-1 text-xs bg-transparent border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]"
            />
            <button
              onClick={handleCreate}
              className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
            >
              Create
            </button>
            <button
              onClick={() => { setAdding(false); setNewName('') }}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              Cancel
            </button>
          </div>
        ) : (
          <button
            onClick={() => setAdding(true)}
            className="w-full text-left px-3 py-2 text-[10px] text-[var(--color-accent)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer transition-colors"
          >
            + Add Layer
          </button>
        )}
        </div>

        {/* Right: Preview panel */}
        <div className="w-64 flex-shrink-0 border border-[var(--color-border)] flex flex-col min-h-[300px]">
          {selectedLayer ? (
            <>
              <div className="px-3 py-2 border-b border-[var(--color-border)] flex items-center justify-between flex-shrink-0">
                <div>
                  <span className="text-xs font-medium text-[var(--color-text-primary)]">{selectedLayer.name}</span>
                  <span className={`ml-2 text-[10px] ${selectedLayer.type === 'locked' ? 'text-[var(--color-text-muted)] italic' : 'text-[var(--color-accent)]'}`}>
                    {selectedLayer.type === 'locked' ? 'auto' : 'custom'}
                  </span>
                </div>
                {selectedLayer.type === 'user' && selectedLayer.filename && (
                  <button
                    onClick={() => {
                      const layer = layers.find((l) => l.filename === selectedLayer.filename)
                      if (layer) handleEdit(layer)
                    }}
                    className="px-2 py-0.5 text-[10px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer"
                  >
                    Edit
                  </button>
                )}
              </div>
              <div className="flex-1 overflow-y-auto px-3 py-2">
                <div className="prose prose-invert prose-xs max-w-none text-xs text-[var(--color-text-secondary)] leading-relaxed">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{previewContent}</ReactMarkdown>
                </div>
              </div>
            </>
          ) : (
            <div className="flex items-center justify-center h-full text-[10px] text-[var(--color-text-muted)]">
              Click a layer to preview
            </div>
          )}
        </div>
      </div>

      {/* Toast */}
      {toast && (
        <div className="mt-3 px-3 py-1.5 text-[10px] text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] inline-block">
          {toast}
        </div>
      )}
    </div>
  )
}

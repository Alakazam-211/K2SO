import React from 'react'
import { useEffect, useState, useCallback, useRef, useMemo } from 'react'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import type { SettingsSection, TerminalSettings } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore } from '@/stores/presets'
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
import DisableWorktreesDialog from './DisableWorktreesDialog'
import { showContextMenu } from '@/lib/context-menu'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { EDITOR_THEMES, EDITOR_FONTS, CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from './CustomThemeCreator'
import { AgentPersonaEditor } from '@/components/AgentPersonaEditor/AgentPersonaEditor'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { KeyCombo } from '@/components/KeySymbol'
import { useClaudeAuthStore } from '@/stores/claude-auth'
import type { ClaudeAuthState } from '@/stores/claude-auth'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import type { UpdateInfo } from '@/hooks/useUpdateChecker'
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
  { id: 'terminal', label: 'Terminal' },
  { id: 'code-editor', label: 'Code Editor' },
  { id: 'editors-agents', label: 'Editors & Agents' },
  { id: 'ai-assistant', label: 'AI Assistant' },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'timer', label: 'Timer' },
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
        {activeSection === 'ai-assistant' && <AIAssistantSection />}
        {activeSection === 'timer' && (
          <SectionErrorBoundary>
            <TimerSection />
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
    <div className="max-w-2xl">
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

      {/* Legend */}
      <div className="flex gap-4 mb-4 text-[11px]">
        <span className="flex items-center gap-1.5"><span className="w-2 h-2 rounded-full bg-green-400" /><span className="text-[var(--color-text-secondary)]">Auto</span><span className="text-[var(--color-text-muted)]">— build and merge automatically</span></span>
        <span className="flex items-center gap-1.5"><span className="w-2 h-2 rounded-full bg-amber-400" /><span className="text-[var(--color-text-secondary)]">Gated</span><span className="text-[var(--color-text-muted)]">— build PRs, wait for approval</span></span>
        <span className="flex items-center gap-1.5"><span className="w-2 h-2 rounded-full bg-[var(--color-text-muted)]" /><span className="text-[var(--color-text-secondary)]">Off</span><span className="text-[var(--color-text-muted)]">— agents don't act</span></span>
      </div>

      {/* State comparison table */}
      <div className="border border-[var(--color-border)] overflow-hidden">
        {/* Header */}
        <div className="grid gap-0 text-[var(--color-text-muted)] bg-[var(--color-bg-surface)]" style={{ gridTemplateColumns: '1fr repeat(5, 90px)' }}>
          <div className="px-4 py-2.5">
            <span className="text-[11px] font-medium">State</span>
          </div>
          {CAPABILITIES.map((cap) => (
            <div key={cap.key} className="px-2 py-2.5 text-center">
              <span className="text-[11px] font-medium block">{cap.label}</span>
              <span className="text-[9px] opacity-60 block mt-0.5">{cap.desc}</span>
            </div>
          ))}
        </div>

        {/* Rows */}
        {states.map((entry) => (
          <div
            key={entry.id}
            className="grid gap-0 border-t border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)]/50 group"
            style={{ gridTemplateColumns: '1fr repeat(5, 90px)' }}
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
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [checking, setChecking] = useState(false)

  // Load current version on mount
  useEffect(() => {
    invoke<string>('get_current_version').then(setCurrentVersion).catch((e) => console.warn('[settings]', e))
  }, [])

  const handleCheckUpdate = useCallback(async () => {
    setChecking(true)
    try {
      const info = await checkForUpdate(true)
      setUpdateInfo(info)
    } finally {
      setChecking(false)
    }
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
            <span className="text-xs text-[var(--color-text-muted)]">
              v{currentVersion || '...'}
            </span>
            <button
              onClick={handleCheckUpdate}
              disabled={checking}
              className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
            >
              {checking ? 'Checking...' : 'Check for Updates'}
            </button>
          </div>
        </div>

        {/* Update available banner */}
        {updateInfo?.has_update && (
          <div className="flex items-center justify-between p-3 bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30">
            <div>
              <p className="text-xs text-[var(--color-text-primary)]">
                K2SO v{updateInfo.latest_version} is available
              </p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                You&apos;re on v{updateInfo.current_version}
              </p>
            </div>
            <button
              className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              onClick={() => invoke('open_external', { url: updateInfo!.download_url }).catch((e) => console.warn('[settings]', e))}
            >
              Download
            </button>
          </div>
        )}

        {/* Agentic Systems master switch */}
        <AgenticSystemsToggle />

        {/* Claude Auth Auto-Refresh */}
        <ClaudeAuthRefreshRow />

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
    docs: 'https://github.com/google-gemini/gemini-cli',
    notes: 'Requires Node.js 18+. Authenticate with your Google account on first run.'
  },
  {
    name: 'GitHub Copilot CLI',
    command: 'copilot',
    installCommand: 'npm install -g @anthropic-ai/copilot-cli',
    docs: 'https://githubnext.com/projects/copilot-cli/',
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
    name: 'Ollama',
    command: 'ollama',
    installCommand: 'curl -fsSL https://ollama.ai/install.sh | sh',
    docs: 'https://ollama.ai',
    notes: 'Run large language models locally. After install, pull a model with "ollama pull llama3".'
  }
]

function K2SOCLIInstall(): React.JSX.Element {
  const [status, setStatus] = useState<{
    installed: boolean
    symlinkPath: string
    target: string | null
    bundledPath: string | null
  } | null>(null)
  const [loading, setLoading] = useState(false)

  const checkStatus = useCallback(async () => {
    try {
      const result = await invoke<{
        installed: boolean
        symlinkPath: string
        target: string | null
        bundledPath: string | null
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
                <div className="w-2 h-2 bg-green-500 flex-shrink-0" />
                <span className="text-xs text-[var(--color-text-secondary)]">Installed</span>
              </div>
              <button
                onClick={handleUninstall}
                disabled={loading}
                className="px-3 py-1.5 text-[11px] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Removing...' : 'Uninstall'}
              </button>
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
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⌘ 1-9' : '⇧⌘ 1-9'} />
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <div>
                    <span className="text-xs text-[var(--color-text-secondary)]">Pinned Workspaces</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] ml-2">1-9</span>
                  </div>
                  <span className="text-xs font-mono text-[var(--color-text-muted)] bg-white/[0.06] px-2 py-0.5">
                    <KeyCombo combo={shortcutLayout === 'cmd-active-cmdshift-pinned' ? '⇧⌘ 1-9' : '⌘ 1-9'} />
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

  const settingsAgenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  const agentPinnedProjects = useMemo(() =>
    settingsAgenticEnabled ? projects.filter((p) => p.agentMode === 'agent' || p.agentMode === 'custom') : [],
    [projects, settingsAgenticEnabled])
  const agentIds = useMemo(() => new Set(agentPinnedProjects.map((p) => p.id)), [agentPinnedProjects])
  const pinnedProjects = useMemo(() => projects.filter((p) => p.pinned && !agentIds.has(p.id)), [projects, agentIds])
  const regularPinnedProjects = pinnedProjects
  const ungroupedProjects = projects.filter((p) => !p.focusGroupId && !p.pinned && !agentIds.has(p.id))
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

  const [disableWorktreeProject, setDisableWorktreeProject] = useState<typeof projects[number] | null>(null)

  // State for git init when enabling worktrees on non-git workspace
  const [gitInitForWorktree, setGitInitForWorktree] = useState<{ projectId: string; projectPath: string; projectName: string } | null>(null)
  const [gitInitBranch, setGitInitBranch] = useState('main')
  const [gitInitPending, setGitInitPending] = useState(false)
  const [gitInitError, setGitInitError] = useState<string | null>(null)

  const handleToggleWorktree = useCallback(async (projectId: string, currentMode: number) => {
    if (currentMode === 1) {
      // Disabling worktrees — check if worktrees exist
      const project = projects.find((p) => p.id === projectId)
      if (project) {
        const worktrees = project.workspaces.filter((ws) => ws.type === 'worktree')
        if (worktrees.length > 0) {
          setDisableWorktreeProject(project)
          return
        }
      }
    } else {
      // Enabling worktrees — check if git is initialized
      const project = projects.find((p) => p.id === projectId)
      if (project) {
        try {
          const gitInfo = await invoke<{ isRepo: boolean; currentBranch?: string }>('git_info', { path: project.path })
          if (!gitInfo.isRepo) {
            // Not a git repo — need to initialize first
            setGitInitForWorktree({ projectId: project.id, projectPath: project.path, projectName: project.name })
            setGitInitBranch('main')
            setGitInitError(null)
            return
          }
        } catch {
          // If we can't check, assume it's fine and proceed
        }
      }
    }
    // Enable/disable normally
    const newMode = currentMode ? 0 : 1
    await invoke('projects_update', { id: projectId, worktreeMode: newMode })
    await fetchProjects()
  }, [fetchProjects, projects])

  const handleGitInitForWorktree = useCallback(async () => {
    if (!gitInitForWorktree) return
    setGitInitPending(true)
    setGitInitError(null)
    try {
      await invoke('projects_init_git_and_open', {
        path: gitInitForWorktree.projectPath,
        branch: gitInitBranch
      })
      // Git initialized — now enable worktrees
      await invoke('projects_update', { id: gitInitForWorktree.projectId, worktreeMode: 1 })
      await fetchProjects()
      setGitInitForWorktree(null)
    } catch (err) {
      setGitInitError(err instanceof Error ? err.message : String(err))
    } finally {
      setGitInitPending(false)
    }
  }, [gitInitForWorktree, gitInitBranch, fetchProjects])

  // Workspace row component (reused in groups and ungrouped)
  const ProjectRow = useCallback(({ project: p, zone, containerSelector }: {
    project: typeof projects[number]
    zone: string
    containerSelector: string
  }) => {
    const isSelected = selectedProjectId === p.id
    const isDragged = reorderDragId === p.id
    return (
      <div
        data-settings-project-id={p.id}
        onClick={() => setSelectedProjectId(p.id)}
        onMouseDown={(e) => handleReorderMouseDown(e, p.id, zone, containerSelector)}
        className={`flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer group select-none ${
          isSelected
            ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
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
            await invoke('projects_update', { id: p.id, pinned: p.pinned ? 0 : 1 })
            await fetchProjects()
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
  }, [selectedProjectId, reorderDragId, handleReorderMouseDown, fetchProjects])

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
                    <ProjectRow project={p} zone="agents" containerSelector="[data-reorder-zone='agents']" />
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
                    <ProjectRow project={p} zone="pinned" containerSelector="[data-reorder-zone='pinned']" />
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
                const groupProjects = projects.filter((p) => p.focusGroupId === group.id && !p.pinned && !agentIds.has(p.id))
                const isCollapsed = collapsedGroups.has(group.id)
                const isDragOver = dragOverGroupId === group.id
                const zoneId = `group:${group.id}`
                const isGroupDragged = groupDragId === group.id
                const showGroupDropBefore = groupDropIndex === groupIdx
                const showGroupDropAfter = groupDropIndex === focusGroups.length && groupIdx === focusGroups.length - 1

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
                            <ProjectRow project={p} zone={zoneId} containerSelector={`[data-reorder-zone='${zoneId}']`} />
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
                        <ProjectRow project={p} zone="ungrouped" containerSelector="[data-reorder-zone='ungrouped']" />
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
                    <ProjectRow project={p} zone="flat" containerSelector="[data-reorder-zone='flat']" />
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
            handleToggleWorktree={handleToggleWorktree}
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

      {/* Disable worktrees dialog */}
      {disableWorktreeProject && (
        <DisableWorktreesDialog
          project={disableWorktreeProject}
          open={true}
          onClose={() => setDisableWorktreeProject(null)}
        />
      )}

      {/* Git init dialog for enabling worktrees on non-git workspace */}
      {gitInitForWorktree && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center no-drag"
          style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
          onClick={gitInitPending ? undefined : () => setGitInitForWorktree(null)}
        >
          <div
            className="w-[440px] border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-5 pt-5 pb-2">
              <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
                Initialize Git to Enable Worktrees
              </h2>
            </div>

            <div className="px-5 pb-4">
              <p className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
                <span className="text-[var(--color-text-primary)] font-medium">{gitInitForWorktree.projectName}</span>{' '}
                doesn't have git initialized. Worktrees require a git repository. Would you like to initialize one?
              </p>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-1.5 break-all">
                {gitInitForWorktree.projectPath}
              </p>
            </div>

            <div className="px-5 pb-4">
              <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">
                Initial branch name
              </label>
              <input
                type="text"
                value={gitInitBranch}
                onChange={(e) => setGitInitBranch(e.target.value)}
                placeholder="main"
                className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]"
                disabled={gitInitPending}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !gitInitPending) handleGitInitForWorktree()
                }}
              />
            </div>

            {gitInitError && (
              <div className="px-5 pb-4">
                <div className="border border-red-500/30 bg-red-500/10 px-3 py-2">
                  <p className="text-[11px] text-red-400 whitespace-pre-wrap">{gitInitError}</p>
                </div>
              </div>
            )}

            <div className="px-5 pb-5 flex items-center justify-end gap-2">
              <button
                className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] border border-[var(--color-border)] transition-colors disabled:opacity-40 no-drag cursor-pointer"
                onClick={() => setGitInitForWorktree(null)}
                disabled={gitInitPending}
              >
                Cancel
              </button>
              <button
                className="px-3 py-1.5 text-xs text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] border border-transparent transition-colors disabled:opacity-40 no-drag cursor-pointer"
                onClick={handleGitInitForWorktree}
                disabled={gitInitPending || !gitInitBranch.trim()}
              >
                {gitInitPending ? 'Initializing...' : 'Initialize Git & Enable Worktrees'}
              </button>
            </div>
          </div>
        </div>
      )}
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
  handleToggleWorktree,
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
  handleToggleWorktree: (projectId: string, currentMode: number) => Promise<void>
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
  const fileInputRef = useRef<HTMLInputElement>(null)


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
          <AgentPersonaEditor
            agentName={agentEditorName}
            projectPath={project.path}
            onClose={() => setAgentEditorOpen(false)}
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
    <div className="max-w-xl space-y-6">
      {/* ── Header ── */}
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">{project.name}</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1 break-all">{project.path}</p>
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
      </SettingsGroup>

      {/* ── Group 2: Agent Settings — Mode tabs, Heartbeat, Agents list ── */}
      {useSettingsStore.getState().agenticSystemsEnabled && <SettingsGroup title="Agent Settings (BETA)">
        <div className="space-y-2">
          {/* Mode selector */}
          <div className="flex gap-1">
            {(['off', 'custom', 'agent', 'pod'] as const).map((mode) => {
              const isActive = (project.agentMode || 'off') === mode
              const labels = { off: 'Off', custom: 'Custom Agent', agent: 'K2SO Agent', pod: 'Pod' }
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
                    } else if (mode === 'pod') {
                      const lines = [
                        'A pod leader delegates work to pod members that execute in parallel worktrees.',
                        '',
                        'What happens:',
                        '• Generates a CLAUDE.md with pod leader instructions',
                        '• A pod-leader agent is created automatically',
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

                    if (mode === 'agent' || mode === 'pod') {
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
            {(project.agentMode || 'off') === 'pod' && 'Pod mode — a pod leader delegates work to pod members that execute in parallel worktrees.'}
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
                  <span className={`text-xs ${project.heartbeatEnabled ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>Heartbeat</span>
                  <p className={`text-[9px] ${project.heartbeatEnabled ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}`}>
                    {project.heartbeatEnabled ? 'Wakes up automatically to work' : 'Only works when manually launched'}
                  </p>
                </div>
              </div>
              <button
                onClick={async () => {
                  const newVal = project.heartbeatEnabled ? 0 : 1

                  // If enabling heartbeat on a K2SO Agent, check for conflicts
                  if (newVal === 1 && (project.agentMode || 'off') === 'agent') {
                    const allProjects = useProjectsStore.getState().projects
                    const otherK2so = allProjects.find(
                      (p) => p.id !== project.id && p.agentMode === 'agent' && p.heartbeatEnabled
                    )
                    if (otherK2so) {
                      const confirmed = await useConfirmDialogStore.getState().confirm({
                        title: 'Move K2SO Heartbeat?',
                        message: `The K2SO Agent heartbeat is currently active in "${otherK2so.name}". Only one K2SO Agent can have an active heartbeat to avoid conflicting autonomous decisions.\n\nMove the heartbeat from "${otherK2so.name}" to "${project.name}"?`,
                        confirmLabel: 'Move Heartbeat',
                      })
                      if (!confirmed) return

                      // Disable heartbeat on the other workspace
                      const store = useProjectsStore.getState()
                      const updated = store.projects.map((p) =>
                        p.id === otherK2so.id ? { ...p, heartbeatEnabled: 0 } : p
                      )
                      useProjectsStore.setState({ projects: updated })
                      await invoke('projects_update', { id: otherK2so.id, heartbeatEnabled: 0 })
                    }
                  }

                  // Update store in-place to avoid full re-render jiggle
                  const store = useProjectsStore.getState()
                  const updatedProjects = store.projects.map((p) =>
                    p.id === project.id ? { ...p, heartbeatEnabled: newVal } : p
                  )
                  useProjectsStore.setState({ projects: updatedProjects })
                  await invoke('projects_update', { id: project.id, heartbeatEnabled: newVal })
                  await invoke('k2so_agents_update_heartbeat_projects').catch(console.error)
                  if (newVal === 1) {
                    await invoke('k2so_agents_install_heartbeat').catch(console.error)
                  }
                }}
                className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
                  project.heartbeatEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-3 h-3 bg-white block transition-transform ${
                    project.heartbeatEnabled ? 'translate-x-4.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>
          )}

          {/* Adaptive Heartbeat Config — only for custom agents with heartbeat enabled */}
          {(project.agentMode || 'off') === 'custom' && project.heartbeatEnabled ? (
            <AdaptiveHeartbeatConfig projectPath={project.path} />
          ) : null}

          {/* Custom Agent persona — only in Custom Agent mode */}
          {(project.agentMode || 'off') === 'custom' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <CustomAgentPersonaButton projectPath={project.path} projectName={project.name} onOpenEditor={(name) => { setAgentEditorName(name); setAgentEditorOpen(true) }} />
            </div>
          )}

          {/* Pod agents list — only in Pod mode */}
          {(project.agentMode || 'off') === 'pod' && (
            <div className="pt-2 border-t border-[var(--color-border)]">
              <ProjectAgentsPanel projectPath={project.path} />
            </div>
          )}
        </div>
      </SettingsGroup>}

      {/* ── Group 3: Worktree Settings — hidden in agent/custom mode (no worktrees needed) ── */}
      {(project.agentMode || 'off') !== 'agent' && (project.agentMode || 'off') !== 'custom' && (
      <SettingsGroup title="Worktree Settings">
        {/* Worktrees toggle */}
        <div className="flex items-center justify-between py-2">
          <div>
            <span className="text-xs text-[var(--color-text-secondary)]">Enable Worktrees</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              {project.worktreeMode ? 'Worktrees use isolated git worktrees' : 'Single worktree using main folder'}
            </p>
          </div>
          <button
            onClick={() => handleToggleWorktree(project.id, project.worktreeMode)}
            className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
              project.worktreeMode ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span
              className={`w-3 h-3 bg-white block transition-transform ${
                project.worktreeMode ? 'translate-x-4.5' : 'translate-x-0.5'
              }`}
            />
          </button>
        </div>

        {/* Worktrees table — uses stableWorktreeMode to prevent layout jiggle */}
        <div className={project.worktreeMode === 1 && project.workspaces.length > 0 ? '' : 'hidden'}>
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

        {/* Worktree Folders on Disk — uses stableWorktreeMode to prevent layout jiggle */}
        <div className={project.worktreeMode === 1 ? '' : 'hidden'}>
          <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />
        </div>
      </SettingsGroup>
      )}

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
  podLeader: boolean
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
      })
      setConfig(result)
    } catch (err) {
      console.error('[heartbeat] Update failed:', err)
    }
  }

  const handleForceWake = async () => {
    try {
      await invoke('k2so_agents_set_heartbeat', {
        projectPath,
        agentName: selectedAgent,
        interval: null,
        phase: null,
        mode: null,
        costBudget: null,
      })
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
        <div>
          <span className="text-xs text-[var(--color-text-primary)]">State</span>
          {activeState?.description && (
            <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">{activeState.description}</p>
          )}
        </div>
        <SettingDropdown
          value={selectedId || ''}
          options={[
            { value: '', label: 'No state' },
            ...states.map((t) => ({ value: t.id, label: t.name })),
          ]}
          onChange={handleChange}
        />
      </div>
      {activeState && (
        <div className="flex gap-3 mt-1.5 text-[9px]">
          {CAPABILITIES.map((cap) => {
            const state = activeState[cap.key] as string
            return (
              <span key={cap.key} className={CAP_COLORS[state] || ''}>
                {cap.label}: {CAP_LABELS[state]}
              </span>
            )
          })}
        </div>
      )}
    </div>
  )
}

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

function ProjectAgentsPanel({ projectPath }: { projectPath: string }): React.JSX.Element {
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

  const podLeader = agents.find((a) => a.podLeader)
  const podMembers = agents.filter((a) => !a.podLeader)
  const totalDelegated = podMembers.reduce((sum, a) => sum + a.inboxCount + a.activeCount, 0)
  const totalDone = podMembers.reduce((sum, a) => sum + a.doneCount, 0)

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
            onClick={() => handleLaunch(agent.name)}
            className="px-2 py-0.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
            title="Launch agent session"
          >
            Launch
          </button>
          <AgentKebabMenu
            onSettings={() => openAgentSettings(agent.name)}
            onDelete={() => handleDelete(agent.name)}
          />
        </div>
      </div>
    </div>
  )

  return (
    <div className="space-y-3">
      {/* Pod Leader section */}
      {podLeader && (
        <div>
          <h3 className="text-[10px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-1">
            Pod Leader
          </h3>
          <div className="border border-[var(--color-accent)]/30">
            <div className="px-3 py-2">
              <div className="flex items-center justify-between">
                <div className="flex-1 min-w-0 mr-3">
                  <div className="flex items-center">
                    <span className="text-xs font-medium text-[var(--color-text-primary)] flex-shrink-0">{podLeader.name}</span>
                    <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 ml-1.5 flex-shrink-0">
                      LEADER
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
                  <p className="text-[10px] text-[var(--color-text-muted)] truncate mt-0.5">{podLeader.role}</p>
                </div>
                <div className="flex items-center gap-1 flex-shrink-0">
                  <button
                    onClick={() => handleLaunch(podLeader.name)}
                    className="px-2 py-0.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
                    title="Launch pod leader session"
                  >
                    Launch
                  </button>
                  <AgentKebabMenu
                    onSettings={() => openAgentSettings(podLeader.name)}
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Pod Members section */}
      <div>
        <div className="flex items-center justify-between mb-1">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
            Agents
            {podMembers.length > 0 && (
              <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{podMembers.length}</span>
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
        ) : podMembers.length === 0 && !showCreate ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            No agents configured. Create one to enable autonomous work.
          </p>
        ) : (
          <div className="border border-[var(--color-border)]">
            {podMembers.map((agent) => (
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
            ? 'AI agents, pods, heartbeat, and review queue are active'
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
    <span className="w-1.5 h-1.5 rounded-full flex-shrink-0" style={{ backgroundColor: color }} />
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

// ── AI Assistant Section ────────────────────────────────────────────

function AIAssistantSection(): React.JSX.Element {
  const { isDownloading, downloadProgress, modelLoaded } = useAssistantStore()
  const aiAssistantEnabled = useSettingsStore((s) => s.aiAssistantEnabled)
  const setAiAssistantEnabled = useSettingsStore((s) => s.setAiAssistantEnabled)
  const [modelPath, setModelPath] = useState<string | null>(null)
  const [modelExists, setModelExists] = useState<boolean | null>(null)
  const [customPath, setCustomPath] = useState('')
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingModel, setLoadingModel] = useState(false)

  // Check model status on mount
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
      // Backend copies the file to ~/.k2so/models/ and returns the final path
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
    <div className="max-w-xl space-y-6">
      {/* ── Local LLM ── */}
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Local LLM</h2>
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

      {/* ── CLI ── */}
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">CLI</h2>
        <p className="text-xs text-[var(--color-text-muted)] mb-4">
          The K2SO CLI lets any AI agent orchestrate your workspaces.
          When you open a terminal inside K2SO, the CLI is automatically available — no setup needed.
        </p>

        {/* CLI Install */}
        <K2SOCLIInstall />

        {/* Using Claude with K2SO */}
        <div className="border border-[var(--color-border)] mt-4">
          <div className="px-4 py-3 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-primary)]">Agent Mode (recommended)</span>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
              Set any workspace to <span className="font-mono text-[var(--color-text-secondary)]">Agent</span> mode in its settings.
              K2SO generates a CLAUDE.md file that teaches Claude everything about the CLI, workspace setup,
              and how to orchestrate work across projects.
            </p>
          </div>
          <div className="px-4 py-3 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-primary)]">External Terminal</span>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
              Install the K2SO CLI above, then use <span className="font-mono text-[var(--color-text-secondary)]">k2so</span> from
              any terminal while K2SO is running.
            </p>
          </div>
          <div className="px-4 py-3 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-primary)]">Key Commands</span>
            <div className="mt-2 bg-[var(--color-bg)] border border-[var(--color-border)] p-3 font-mono text-[11px] text-[var(--color-text-secondary)] leading-relaxed space-y-1">
              <div><span className="text-[var(--color-accent)]">k2so agents list</span>          <span className="text-[var(--color-text-muted)]"># List all agents</span></div>
              <div><span className="text-[var(--color-accent)]">k2so delegate</span> &lt;agent&gt; &lt;file&gt;  <span className="text-[var(--color-text-muted)]"># Assign work (creates worktree + launches)</span></div>
              <div><span className="text-[var(--color-accent)]">k2so work create</span> --title &quot;...&quot; <span className="text-[var(--color-text-muted)]"># Create a work item</span></div>
              <div><span className="text-[var(--color-accent)]">k2so reviews</span>               <span className="text-[var(--color-text-muted)]"># See pending reviews</span></div>
              <div><span className="text-[var(--color-accent)]">k2so review approve</span> &lt;a&gt; &lt;b&gt; <span className="text-[var(--color-text-muted)]"># Merge + cleanup</span></div>
              <div><span className="text-[var(--color-accent)]">k2so mode pod</span>              <span className="text-[var(--color-text-muted)]"># Enable pod mode</span></div>
              <div><span className="text-[var(--color-accent)]">k2so worktree on</span>           <span className="text-[var(--color-text-muted)]"># Enable worktrees</span></div>
              <div><span className="text-[var(--color-accent)]">k2so heartbeat on</span>          <span className="text-[var(--color-text-muted)]"># Enable auto heartbeat</span></div>
              <div><span className="text-[var(--color-accent)]">k2so settings</span>              <span className="text-[var(--color-text-muted)]"># Show workspace settings</span></div>
              <div><span className="text-[var(--color-accent)]">k2so help</span>                  <span className="text-[var(--color-text-muted)]"># Full command reference</span></div>
            </div>
          </div>
          <div className="px-4 py-3">
            <span className="text-xs text-[var(--color-text-primary)]">Connection Details</span>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
              The CLI communicates via a local HTTP server.
              <span className="font-mono text-[var(--color-text-secondary)]"> K2SO_PORT</span> and
              <span className="font-mono text-[var(--color-text-secondary)]"> K2SO_HOOK_TOKEN</span> are
              set automatically in K2SO terminals. For external terminals, connection details are at
              <span className="font-mono text-[var(--color-text-secondary)]"> ~/.k2so/heartbeat.port</span> and
              <span className="font-mono text-[var(--color-text-secondary)]"> ~/.k2so/heartbeat.token</span>.
            </p>
          </div>
        </div>
      </div>
    </div>
  )
}

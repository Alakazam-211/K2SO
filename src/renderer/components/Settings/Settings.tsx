import { useEffect, useState, useCallback, useRef } from 'react'
import { useSettingsStore, getEffectiveKeybinding } from '@/stores/settings'
import type { SettingsSection, TerminalSettings } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePresetsStore } from '@/stores/presets'
import { trpc } from '@/lib/trpc'
import {
  HOTKEYS,
  RESERVED_KEYS,
  formatKeyCombo,
  keyEventToCombo,
  isReservedKey
} from '@shared/hotkeys'
import type { HotkeyDefinition } from '@shared/hotkeys'
import DisableWorktreesDialog from './DisableWorktreesDialog'

// ── Section nav items ────────────────────────────────────────────────
const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: 'general', label: 'General' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'editors-agents', label: 'Editors & Agents' },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'projects', label: 'Workspaces' }
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
    <div className="flex h-full w-full bg-[var(--color-bg)]">
      {/* Left nav */}
      <div className="w-48 flex-shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-surface)] flex flex-col">
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
          <span className="text-xs font-medium text-[var(--color-text-secondary)] uppercase tracking-wider">
            Settings
          </span>
        </div>
        <nav className="flex-1 py-1">
          {SECTIONS.map((s) => (
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
        <div className="px-4 py-3 border-t border-[var(--color-border)]">
          <button
            onClick={closeSettings}
            className="text-xs text-[var(--color-text-primary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          >
            &larr; Back
          </button>
        </div>
      </div>

      {/* Content area */}
      <div className="flex-1 overflow-y-auto p-6">
        {activeSection === 'general' && <GeneralSection />}
        {activeSection === 'terminal' && <TerminalSection />}
        {activeSection === 'editors-agents' && <EditorsAgentsSection />}
        {activeSection === 'keybindings' && <KeybindingsSection />}
        {activeSection === 'projects' && <ProjectsSection />}
      </div>
    </div>
  )
}

// ── General Section ──────────────────────────────────────────────────
function GeneralSection(): React.JSX.Element {
  const resetAllSettings = useSettingsStore((s) => s.resetAllSettings)
  const [confirming, setConfirming] = useState(false)

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">General</h2>

      <div className="space-y-4">
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">App Version</span>
          <span className="text-xs text-[var(--color-text-muted)]">0.1.0</span>
        </div>

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
              className="w-40 no-drag accent-[var(--color-accent)]"
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
          <select
            value={terminal.cursorStyle}
            onChange={(e) =>
              updateTerminalSettings({
                cursorStyle: e.target.value as TerminalSettings['cursorStyle']
              })
            }
            className="w-40 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer"
          >
            <option value="bar">Bar</option>
            <option value="block">Block</option>
            <option value="underline">Underline</option>
          </select>
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

function EditorsAgentsSection(): React.JSX.Element {
  const { presets, fetchPresets } = usePresetsStore()
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
  const formLabelRef = useRef<HTMLInputElement>(null)

  const loadEditors = useCallback(async () => {
    setEditorsLoading(true)
    try {
      const result = await trpc.projects.getAllEditors.query()
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
      const result = await trpc.projects.refreshEditors.mutate()
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
    await trpc.presets.update.mutate({ id, enabled: currentEnabled ? 0 : 1 })
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
      await trpc.presets.delete.mutate({ id })
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
        await trpc.presets.update.mutate({
          id: presetForm.editingId,
          label: presetForm.label.trim(),
          command: presetForm.command.trim(),
          icon: presetForm.icon.trim() || undefined
        })
      } else {
        await trpc.presets.create.mutate({
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
    await trpc.presets.resetBuiltIns.mutate()
    fetchPresets()
  }, [fetchPresets])

  const handleDragStart = useCallback((idx: number) => {
    setDragIdx(idx)
  }, [])

  const handleDragOver = useCallback((e: React.DragEvent, idx: number) => {
    e.preventDefault()
    setDragOverIdx(idx)
  }, [])

  const handleDrop = useCallback(async (targetIdx: number) => {
    if (dragIdx === null || dragIdx === targetIdx) {
      setDragIdx(null)
      setDragOverIdx(null)
      return
    }

    const sorted = [...presets]
    const [moved] = sorted.splice(dragIdx, 1)
    sorted.splice(targetIdx, 0, moved)

    const ids = sorted.map((p) => p.id)
    await trpc.presets.reorder.mutate({ ids })
    fetchPresets()

    setDragIdx(null)
    setDragOverIdx(null)
  }, [dragIdx, presets, fetchPresets])

  const editorApps = editors.filter((e) => e.type === 'editor')
  const terminalApps = editors.filter((e) => e.type === 'terminal')

  return (
    <div className="max-w-2xl space-y-8">
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

        <div className="border border-[var(--color-border)]">
          {presets.map((preset, i) => (
            <div
              key={preset.id}
              draggable
              onDragStart={() => handleDragStart(i)}
              onDragOver={(e) => handleDragOver(e, i)}
              onDrop={() => handleDrop(i)}
              onDragEnd={() => { setDragIdx(null); setDragOverIdx(null) }}
              className={`flex items-center gap-2 px-3 py-1.5 group transition-colors ${
                i < presets.length - 1 ? 'border-b border-[var(--color-border)]' : ''
              } ${dragIdx === i ? 'opacity-30' : ''} ${
                dragOverIdx === i ? 'bg-[var(--color-accent)]/10' : ''
              } cursor-grab active:cursor-grabbing`}
            >
              {/* Drag handle */}
              <svg
                width="6" height="10" viewBox="0 0 6 10"
                fill="currentColor"
                className="flex-shrink-0 opacity-0 group-hover:opacity-40 transition-opacity text-[var(--color-text-muted)]"
              >
                <circle cx="1.5" cy="1.5" r="1" />
                <circle cx="4.5" cy="1.5" r="1" />
                <circle cx="1.5" cy="5" r="1" />
                <circle cx="4.5" cy="5" r="1" />
                <circle cx="1.5" cy="8.5" r="1" />
                <circle cx="4.5" cy="8.5" r="1" />
              </svg>

              {/* Icon */}
              <span className="text-sm leading-none w-5 text-center flex-shrink-0">
                {preset.icon || '-'}
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
    </div>
  )
}

// ── Keybindings Section ──────────────────────────────────────────────
function KeybindingsSection(): React.JSX.Element {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const updateKeybinding = useSettingsStore((s) => s.updateKeybinding)
  const resetKeybinding = useSettingsStore((s) => s.resetKeybinding)
  const resetAllKeybindings = useSettingsStore((s) => s.resetAllKeybindings)
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

      <div className="space-y-4">
        {grouped.map(({ category, items }) => (
          <div key={category}>
            <div className="text-xs font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1 px-1">
              {category}
            </div>
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
            {formatKeyCombo(combo)}
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

  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(
    projects.length > 0 ? projects[0].id : null
  )
  const [newGroupName, setNewGroupName] = useState('')
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set())
  const [dragProjectId, setDragProjectId] = useState<string | null>(null)
  const [dragOverGroupId, setDragOverGroupId] = useState<string | null>(null)

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null
  const editors = ['Cursor', 'VS Code', 'Zed', 'Other']
  const ungroupedProjects = projects.filter((p) => !p.focusGroupId)

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

  const [disableWorktreeProject, setDisableWorktreeProject] = useState<typeof projects[number] | null>(null)

  const handleToggleWorktree = useCallback(async (projectId: string, currentMode: number) => {
    if (currentMode === 1) {
      // Disabling worktrees — check if worktrees exist
      const project = projects.find((p) => p.id === projectId)
      if (project) {
        const worktrees = project.workspaces.filter((ws) => ws.type === 'worktree')
        if (worktrees.length > 0) {
          // Show the dialog
          setDisableWorktreeProject(project)
          return
        }
      }
    }
    // Enabling, or no worktrees to worry about
    const newMode = currentMode ? 0 : 1
    const { trpc } = await import('@/lib/trpc')
    await trpc.projects.update.mutate({ id: projectId, worktreeMode: newMode })
    await fetchProjects()
  }, [fetchProjects, projects])

  // Workspace row component (reused in groups and ungrouped)
  const ProjectRow = useCallback(({ project: p }: { project: typeof projects[number] }) => {
    const isSelected = selectedProjectId === p.id
    const isDragged = dragProjectId === p.id
    return (
      <div
        draggable={focusGroupsEnabled}
        onDragStart={(e) => {
          setDragProjectId(p.id)
          e.dataTransfer.effectAllowed = 'move'
          e.dataTransfer.setData('text/plain', p.id)
          // Use the current target as drag image
          if (e.currentTarget instanceof HTMLElement) {
            e.dataTransfer.setDragImage(e.currentTarget, 10, 10)
          }
        }}
        onDragEnd={() => { setDragProjectId(null); setDragOverGroupId(null) }}
        onClick={() => setSelectedProjectId(p.id)}
        className={`flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer group ${
          isSelected
            ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        } ${isDragged ? 'opacity-30' : ''} ${focusGroupsEnabled ? 'cursor-grab active:cursor-grabbing' : ''}`}
      >
        {/* Drag handle dots — only visible when focus groups enabled */}
        {focusGroupsEnabled && (
          <svg
            width="6" height="10" viewBox="0 0 6 10"
            fill="currentColor"
            className="flex-shrink-0 opacity-0 group-hover:opacity-40 transition-opacity"
          >
            <circle cx="1.5" cy="1.5" r="1" />
            <circle cx="4.5" cy="1.5" r="1" />
            <circle cx="1.5" cy="5" r="1" />
            <circle cx="4.5" cy="5" r="1" />
            <circle cx="1.5" cy="8.5" r="1" />
            <circle cx="4.5" cy="8.5" r="1" />
          </svg>
        )}
        <div
          className="w-5 h-5 flex items-center justify-center flex-shrink-0 text-[10px] font-bold text-white"
          style={{ backgroundColor: p.color }}
        >
          {p.name.charAt(0).toUpperCase()}
        </div>
        <span className="text-xs truncate flex-1">{p.name}</span>
        <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0 tabular-nums">
          {p.worktreeMode ? `${p.workspaces.length}` : 'local'}
        </span>
      </div>
    )
  }, [selectedProjectId, dragProjectId, focusGroupsEnabled])

  return (
    <div className="flex h-full -m-6">
      {/* ── Left panel: focus group toggle + organized workspace list ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col overflow-hidden">
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

        {/* Workspace list — organized by group folders when enabled */}
        <div className="flex-1 overflow-y-auto px-1 py-1">
          {focusGroupsEnabled ? (
            <>
              {/* Focus group folders */}
              {focusGroups.map((group) => {
                const groupProjects = projects.filter((p) => p.focusGroupId === group.id)
                const isCollapsed = collapsedGroups.has(group.id)
                const isDragOver = dragOverGroupId === group.id

                return (
                  <div key={group.id} className="mb-0.5">
                    {/* Group folder header */}
                    <div
                      className={`flex items-center gap-1.5 px-2 py-1 cursor-pointer no-drag transition-colors ${
                        isDragOver ? 'bg-[var(--color-accent)]/10' : 'hover:bg-white/[0.03]'
                      }`}
                      onClick={() => toggleGroupCollapse(group.id)}
                      onDragOver={(e) => { e.preventDefault(); setDragOverGroupId(group.id) }}
                      onDragLeave={() => setDragOverGroupId(null)}
                      onDrop={(e) => { e.preventDefault(); handleDrop(group.id) }}
                    >
                      {/* Color bar */}
                      {group.color && (
                        <span className="w-1 h-3 flex-shrink-0" style={{ backgroundColor: group.color }} />
                      )}
                      {/* Chevron */}
                      <svg
                        className={`w-2.5 h-2.5 text-[var(--color-text-muted)] transition-transform flex-shrink-0 ${
                          isCollapsed ? '' : 'rotate-90'
                        }`}
                        fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
                      >
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
                      </svg>
                      <span className="text-[11px] font-medium text-[var(--color-text-secondary)] flex-1 truncate">
                        {group.name}
                      </span>
                      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                        {groupProjects.length}
                      </span>
                    </div>

                    {/* Group workspaces */}
                    {!isCollapsed && (
                      <div className="ml-3">
                        {groupProjects.map((p) => (
                          <ProjectRow key={p.id} project={p} />
                        ))}
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
                  </div>
                )
              })}

              {/* Ungrouped workspaces */}
              {ungroupedProjects.length > 0 && (
                <div className="mt-1">
                  <div
                    className={`flex items-center gap-1.5 px-2 py-1 text-[11px] font-medium text-[var(--color-text-muted)] transition-colors ${
                      dragOverGroupId === '__ungrouped__' ? 'bg-[var(--color-accent)]/10' : ''
                    }`}
                    onDragOver={(e) => { e.preventDefault(); setDragOverGroupId('__ungrouped__') }}
                    onDragLeave={() => setDragOverGroupId(null)}
                    onDrop={(e) => { e.preventDefault(); handleDrop(null) }}
                  >
                    Ungrouped
                  </div>
                  <div className="ml-1">
                    {ungroupedProjects.map((p) => (
                      <ProjectRow key={p.id} project={p} />
                    ))}
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
              {projects.map((p) => (
                <ProjectRow key={p.id} project={p} />
              ))}
              {projects.length === 0 && (
                <div className="px-2 py-6 text-center">
                  <span className="text-xs text-[var(--color-text-muted)]">No workspaces</span>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      {/* ── Right panel: selected workspace settings ── */}
      <div className="flex-1 overflow-y-auto p-6">
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
    trpc.git.worktrees
      .query({ path: project.path })
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
      await trpc.git.reopenWorktree.mutate({
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
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
        Worktree Folders on Disk ({nonBare.length})
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
  projectSettings: Record<string, { defaultEditor?: string }>
  updateProjectSetting: (projectId: string, editor: string) => void
  removeProject: (id: string) => Promise<void>
  assignProjectToGroup: (projectId: string, groupId: string | null) => Promise<void>
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [iconLoading, setIconLoading] = useState(false)

  const handleDetectIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await trpc.projects.detectIcon.mutate({ projectId: project.id })
      await fetchProjects()
    } catch (err) {
      console.error('Icon detection failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleUploadIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await trpc.projects.uploadIcon.mutate({ projectId: project.id })
      await fetchProjects()
    } catch (err) {
      console.error('Icon upload failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const handleClearIcon = async (): Promise<void> => {
    setIconLoading(true)
    try {
      await trpc.projects.clearIcon.mutate({ projectId: project.id })
      await fetchProjects()
    } catch (err) {
      console.error('Icon clear failed:', err)
    } finally {
      setIconLoading(false)
    }
  }

  const firstLetter = project.name.charAt(0).toUpperCase()

  return (
    <div className="max-w-xl space-y-6">
      {/* ── Header ── */}
      <div>
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">{project.name}</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1 break-all">{project.path}</p>
      </div>

      {/* ── Icon ── */}
      <div className="space-y-3">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Icon
        </h3>
        <div className="flex items-center gap-4">
          {/* Current icon preview */}
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
                className="object-contain"
                style={{ width: 44, height: 44, display: 'block' }}
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

          {/* Action buttons */}
          <div className="flex items-center gap-2">
            <button
              onClick={handleDetectIcon}
              disabled={iconLoading}
              className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              {iconLoading ? 'Working...' : 'Detect'}
            </button>
            <button
              onClick={handleUploadIcon}
              disabled={iconLoading}
              className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              Upload
            </button>
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
      </div>

      {/* ── Workspace Settings ── */}
      <div className="space-y-3">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Workspace
        </h3>

        {/* Worktrees toggle */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
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

        {/* Default editor */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Default Editor</span>
          <select
            value={projectSettings[project.id]?.defaultEditor ?? 'Cursor'}
            onChange={(e) => updateProjectSetting(project.id, e.target.value)}
            className="px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer"
          >
            {editors.map((ed) => (
              <option key={ed} value={ed}>{ed}</option>
            ))}
          </select>
        </div>

        {/* Color */}
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-secondary)]">Workspace Color</span>
          <div className="flex items-center gap-1.5">
            {['#3b82f6', '#ef4444', '#22c55e', '#f59e0b', '#a855f7', '#ec4899', '#06b6d4', '#64748b'].map((color) => (
              <button
                key={color}
                onClick={async () => {
                  const { trpc } = await import('@/lib/trpc')
                  await trpc.projects.update.mutate({ id: project.id, color })
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

        {/* Focus group assignment */}
        {focusGroupsEnabled && (
          <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-secondary)]">Focus Group</span>
            <select
              value={project.focusGroupId ?? ''}
              onChange={async (e) => {
                await assignProjectToGroup(project.id, e.target.value || null)
                await fetchProjects()
              }}
              className="px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer"
            >
              <option value="">No Group</option>
              {focusGroups.map((g) => (
                <option key={g.id} value={g.id}>{g.name}</option>
              ))}
            </select>
          </div>
        )}
      </div>

      {/* ── Worktrees ── */}
      {project.worktreeMode === 1 && project.workspaces.length > 0 && (
        <div className="space-y-2">
          <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
            Worktrees ({project.workspaces.length})
          </h3>
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
      )}

      {/* ── Worktree Folders on Disk ── */}
      {project.worktreeMode === 1 && (
        <WorktreeFoldersOnDisk project={project} fetchProjects={fetchProjects} />
      )}

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
  )
}

// ── Shared components ────────────────────────────────────────────────
function SettingRow({
  label,
  children
}: {
  label: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <span className="text-xs text-[var(--color-text-secondary)]">{label}</span>
      {children}
    </div>
  )
}

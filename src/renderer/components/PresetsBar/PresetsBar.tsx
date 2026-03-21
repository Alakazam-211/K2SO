import { useEffect, useState, useRef, useCallback, useMemo } from 'react'
import { usePresetsStore } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { invoke } from '@tauri-apps/api/core'
import { showContextMenu } from '@/lib/context-menu'
import AgentIcon from '@/components/AgentIcon/AgentIcon'

interface PresetsBarProps {
  cwd: string
}

interface InlineFormState {
  visible: boolean
  editingId: string | null
  label: string
  command: string
  icon: string
}

/**
 * Determine which preset commands are currently running in any terminal pane.
 * Returns a Set of preset IDs that have a matching command in the tabs store.
 */
function useRunningPresetIds(): Set<string> {
  const tabs = useTabsStore((s) => s.tabs)
  const presets = usePresetsStore((s) => s.presets)

  return useMemo(() => {
    const running = new Set<string>()

    // Collect all active terminal commands across all tabs/paneGroups
    const activeCommands = new Set<string>()
    for (const tab of tabs) {
      if (!tab.paneGroups) continue
      for (const [, pg] of tab.paneGroups) {
        if (!pg?.items) continue
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as { command?: string }
            if (data.command) {
              activeCommands.add(data.command)
            }
          }
        }
      }
    }

    for (const preset of presets) {
      // Extract the base command (first token) from the preset
      const baseCommand = preset.command.split(/\s+/)[0]
      if (baseCommand && activeCommands.has(baseCommand)) {
        running.add(preset.id)
      }
    }

    return running
  }, [tabs, presets])
}

export function PresetsBar({ cwd }: PresetsBarProps): React.JSX.Element | null {
  const { presets, showPresetsBar, fetchPresets, launchPreset } = usePresetsStore()
  const runningIds = useRunningPresetIds()

  const [form, setForm] = useState<InlineFormState>({
    visible: false,
    editingId: null,
    label: '',
    command: '',
    icon: ''
  })

  const [hoveredId, setHoveredId] = useState<string | null>(null)

  const formLabelRef = useRef<HTMLInputElement>(null)

  // Fetch presets on mount
  useEffect(() => {
    fetchPresets()
  }, [fetchPresets])

  // Focus label input when form opens
  useEffect(() => {
    if (form.visible) {
      requestAnimationFrame(() => formLabelRef.current?.focus())
    }
  }, [form.visible])

  const enabledPresets = presets.filter((p) => p.enabled)

  const handleClick = useCallback(
    (presetId: string) => {
      launchPreset(presetId, cwd, 'tab')
    },
    [launchPreset, cwd]
  )

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, presetId: string) => {
      e.preventDefault()

      const menuItems = [
        { id: 'tab', label: 'Open in New Tab' },
        { id: 'split', label: 'Split Pane' },
        { id: 'separator', label: '', type: 'separator' as const },
        { id: 'edit', label: 'Edit' },
        { id: 'disable', label: 'Disable' }
      ]

      const clickedId = await showContextMenu(menuItems)
      if (!clickedId) return

      switch (clickedId) {
        case 'tab':
          launchPreset(presetId, cwd, 'tab')
          break
        case 'split':
          launchPreset(presetId, cwd, 'split')
          break
        case 'edit': {
          const preset = presets.find((p) => p.id === presetId)
          if (preset) {
            setForm({
              visible: true,
              editingId: presetId,
              label: preset.label,
              command: preset.command,
              icon: preset.icon ?? ''
            })
          }
          break
        }
        case 'disable': {
          await invoke('presets_update', { id: presetId, enabled: 0 })
          fetchPresets()
          break
        }
      }
    },
    [presets, launchPreset, cwd, fetchPresets]
  )

  const openNewForm = useCallback(() => {
    setForm({ visible: true, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const cancelForm = useCallback(() => {
    setForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const submitForm = useCallback(async () => {
    if (!form.label.trim() || !form.command.trim()) return

    try {
      if (form.editingId) {
        await invoke('presets_update', {
          id: form.editingId,
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || undefined
        })
      } else {
        await invoke('presets_create', {
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || undefined
        })
      }
      cancelForm()
      fetchPresets()
    } catch (err) {
      console.error('Failed to save preset:', err)
    }
  }, [form, cancelForm, fetchPresets])

  const handleFormKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        submitForm()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        cancelForm()
      }
    },
    [submitForm, cancelForm]
  )

  if (!showPresetsBar) return null

  return (
    <div
      style={{
        position: 'relative',
        display: 'flex',
        alignItems: 'center',
        gap: '1px',
        borderBottom: '1px solid var(--color-border, #1e1e1e)',
        backgroundColor: '#0a0a0a',
        paddingLeft: '4px',
        paddingRight: '4px',
        height: '32px',
        fontFamily: 'var(--font-mono, ui-monospace, monospace)',
        flexShrink: 0,
      }}
    >
      {/* Preset buttons */}
      {enabledPresets.map((preset) => {
        const isRunning = runningIds.has(preset.id)
        const isHovered = hoveredId === preset.id

        return (
          <button
            key={preset.id}
            onClick={() => handleClick(preset.id)}
            onContextMenu={(e) => handleContextMenu(e, preset.id)}
            onMouseEnter={() => setHoveredId(preset.id)}
            onMouseLeave={() => setHoveredId(null)}
            title={preset.command}
            style={{
              position: 'relative',
              display: 'flex',
              alignItems: 'center',
              gap: '6px',
              height: '26px',
              paddingLeft: '8px',
              paddingRight: '10px',
              border: 'none',
              outline: 'none',
              cursor: 'pointer',
              fontSize: '11px',
              fontFamily: 'inherit',
              letterSpacing: '0.02em',
              whiteSpace: 'nowrap',
              transition: 'background-color 120ms ease, color 120ms ease',
              backgroundColor: isRunning
                ? (isHovered ? '#1a2a1a' : '#111a11')
                : (isHovered ? '#1a1a1a' : 'transparent'),
              color: isRunning
                ? '#70c070'
                : (isHovered ? '#e0e0e0' : '#808080'),
              borderLeft: isRunning ? '2px solid #4ade80' : '2px solid transparent',
            }}
          >
            <AgentIcon agent={preset.label} size={14} />
            <span style={{ lineHeight: 1 }}>{preset.label}</span>
            {isRunning && (
              <span
                style={{
                  display: 'inline-block',
                  width: '5px',
                  height: '5px',
                  backgroundColor: '#4ade80',
                  borderRadius: '50%',
                  marginLeft: '2px',
                  flexShrink: 0,
                }}
              />
            )}
          </button>
        )
      })}

      {/* Add button */}
      <button
        onClick={openNewForm}
        onMouseEnter={(e) => {
          ;(e.currentTarget as HTMLButtonElement).style.color = '#e0e0e0'
          ;(e.currentTarget as HTMLButtonElement).style.backgroundColor = '#1a1a1a'
        }}
        onMouseLeave={(e) => {
          ;(e.currentTarget as HTMLButtonElement).style.color = '#505050'
          ;(e.currentTarget as HTMLButtonElement).style.backgroundColor = 'transparent'
        }}
        title="Add agent preset"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: '26px',
          height: '26px',
          border: 'none',
          outline: 'none',
          cursor: 'pointer',
          fontSize: '14px',
          fontFamily: 'inherit',
          color: '#505050',
          backgroundColor: 'transparent',
          transition: 'background-color 120ms ease, color 120ms ease',
          marginLeft: '2px',
        }}
      >
        +
      </button>

      {/* Inline form */}
      {form.visible && (
        <div
          onKeyDown={handleFormKeyDown}
          style={{
            position: 'absolute',
            left: 0,
            top: '100%',
            zIndex: 50,
            display: 'flex',
            alignItems: 'center',
            gap: '4px',
            borderBottom: '1px solid var(--color-border, #1e1e1e)',
            backgroundColor: '#111111',
            padding: '6px 8px',
            boxShadow: '0 4px 12px rgba(0,0,0,0.5)',
            fontFamily: 'inherit',
          }}
        >
          <input
            ref={formLabelRef}
            type="text"
            value={form.icon}
            onChange={(e) => setForm((s) => ({ ...s, icon: e.target.value }))}
            placeholder="Icon"
            style={{
              height: '26px',
              width: '40px',
              border: '1px solid #2a2a2a',
              backgroundColor: '#0a0a0a',
              padding: '0 4px',
              textAlign: 'center',
              fontSize: '11px',
              fontFamily: 'inherit',
              color: '#e0e0e0',
              outline: 'none',
            }}
          />
          <input
            type="text"
            value={form.label}
            onChange={(e) => setForm((s) => ({ ...s, label: e.target.value }))}
            placeholder="Label"
            style={{
              height: '26px',
              width: '80px',
              border: '1px solid #2a2a2a',
              backgroundColor: '#0a0a0a',
              padding: '0 6px',
              fontSize: '11px',
              fontFamily: 'inherit',
              color: '#e0e0e0',
              outline: 'none',
            }}
          />
          <input
            type="text"
            value={form.command}
            onChange={(e) => setForm((s) => ({ ...s, command: e.target.value }))}
            placeholder="Command (e.g. aider --model gpt-4)"
            style={{
              height: '26px',
              width: '220px',
              border: '1px solid #2a2a2a',
              backgroundColor: '#0a0a0a',
              padding: '0 6px',
              fontSize: '11px',
              fontFamily: 'inherit',
              color: '#e0e0e0',
              outline: 'none',
            }}
          />
          <button
            onClick={submitForm}
            style={{
              height: '26px',
              backgroundColor: '#3b82f6',
              padding: '0 8px',
              fontSize: '11px',
              fontFamily: 'inherit',
              color: '#ffffff',
              border: 'none',
              cursor: 'pointer',
            }}
          >
            {form.editingId ? 'Save' : 'Add'}
          </button>
          <button
            onClick={cancelForm}
            style={{
              height: '26px',
              padding: '0 8px',
              fontSize: '11px',
              fontFamily: 'inherit',
              color: '#808080',
              backgroundColor: 'transparent',
              border: 'none',
              cursor: 'pointer',
            }}
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  )
}
